# ADR-0005: Metrics ingest + the one-binary backend — curated store, kinded tokens

- **Status:** Accepted
- **Date:** 2026-07-31
- **Related:** healthie-267 (M2 backend + HAE ingest), ADR-0002 (HAE ingestion +
  quarantine posture), ADR-0003 (typed vocabularies), ADR-0004 (§5 quarantine
  discipline, the conversational-surface counterpart to this server-side one).
  Code: `healthie-shared/src/entities/{daily_metric.rs,quarantined_metric.rs,auth_token.rs}`,
  `healthie-shared/src/services/{metrics.rs,auth_token.rs}`, `healthie-backend/`,
  `healthie-mcp/src/lib.rs`. Deferred to M2b: healthie-hf2 (summarize_trends,
  goal-progress briefings, metric recommendations, sleep timestamps).

## Context

M2 turns healthie into a single deployed binary and gives it its first
non-conversational intake: Apple Health metrics arrive as JSON from the Health
Auto Export (HAE) app over a REST endpoint, on a schedule, with no interactive
caller. ADR-0002 already committed to ingesting HAE and to quarantining anything
unrecognized; what it left open is the _shape_ of the stored metric, which of
Apple's ~250 metric names we actually keep, how a push with no retry loop handles
the unknown, and how the interim `healthie-mcp` binary folds into the real
backend. This ADR records those calls.

## Decision

### 1. `daily_metric`: a curated typed row, not a JSON blob

One row per `(kind, date)` (`UNIQUE`), with `value: f64`, optional `min`/`max`
(the aggregate spread HAE sends for metrics like heart rate), an optional
`source`, and timestamps. `kind` is a closed `MetricKind` vocabulary in the
ADR-0003 house style (`DeriveActiveEnum`, kebab-case serde, `EnumIter`).

- **Why not a per-day JSON document:** the briefing assembler and M2b's trend
  tools need to filter and aggregate by metric and date range. A typed
  `(kind, date, value)` row is directly queryable; a JSON blob would push parsing
  and shape-guessing into every reader.
- **Why `min`/`max` are columns, not derived:** HAE reports them for aggregate
  metrics and they are not recomputable from a daily `value` alone, so they are
  stored where they arrive.

### 2. Broad-curate now, recommend later

`MetricKind` enumerates the metrics worth keeping today (~19 HAE source names,
plus sleep — see §3), chosen for breadth over a minimal set. Deciding _which_
metrics deserve attention, goals, or recommendations is deferred to M2b: the
store keeps the data broadly now so the recommendation layer has history to reason
over when it lands. Promoting a metric later is a code change (a new `MetricKind`
variant + a mapping arm), not a migration of lost data.

### 3. Sleep explodes 1 → many; everything else is scalar or aggregate

Apple's `sleep_analysis` carries several stage figures in one point
(`totalSleep`, `deep`, `rem`, `core`, `awake`, `inBed`). Rather than store an
opaque sleep object, ingest **explodes** it into one `daily_metric` row per
present stage (`SleepTotal`, `SleepDeep`, …, `TimeInBed`) — a stage field that is
absent produces no row (skipped, not zeroed). This keeps every sleep stage a
first-class, queryable `MetricKind` on the same `(kind, date)` contract as
scalar and aggregate metrics. The 1→6 fan-out is why the curated ~19 source
names yield 25 `MetricKind` variants.

### 4. Three buckets: curated, excluded, quarantined — quarantine stays exceptional

Every incoming HAE metric name is classified into exactly one of three buckets:

- **Curated** → mapped to a `MetricKind` and stored.
- **Excluded** → a name we have seen and deliberately do not track
  (`EXCLUDED_HAE_NAMES`), silently ignored. This keeps quarantine _exceptional_:
  only genuinely new names land there.
- **Quarantined** → any name never seen before is written verbatim to
  `quarantined_metric` (`raw_name`, `date`, `raw_point` JSON) — **never dropped**.

The push has no interactive caller to correct and resend (unlike the MCP surface
in ADR-0004 §5), so the unknown must be caught server-side and preserved for later
inspection rather than rejected. Promoting a quarantined name means moving it from
the exclude list / the unknown default into the mapping — the raw points are still
on disk to backfill from.

### 5. Idempotent upsert on local calendar day

Ingest is one transaction; `(kind, date)` and `(raw_name, date)` **upsert
last-write-wins**, so a re-pushed or overlapping export re-lands cleanly instead
of duplicating. HAE stamps each point with a **local** offset (e.g. `-0700`); the
metric belongs to that local calendar day, so ingest parses the offset and takes
the local date — it never UTC-converts, which would shift a late-evening reading
onto the wrong day.

### 6. Kinded `auth_token`: distinct rows, distinct blast radius

M1b's singleton `mcp_token` generalizes to `auth_token` with a `TokenKind`
(`mcp`, `ingest`), `UNIQUE(kind)` — one row per kind. `provision` / `verify` /
`revoke` are all scoped to a kind. The two tokens share the argon2id-at-rest
machinery but are **distinct rows**: a leaked ingest token can never drive MCP
tools, and revoking one kind leaves the other intact. This separation is proven
at the HTTP boundary (an MCP-kind token presented to `/ingest/hae` is rejected),
not just in the service.

### 7. One binary: `healthie-mcp` drops to a library

`healthie-backend` becomes the single deployed binary and owns `main.rs`, the CLI
(`serve` | `token --kind <k> <action>`), and the axum router; `healthie-mcp`
retires its interim binary and becomes a library exposing `router()`, mounted at
`/mcp`.

- **Config lives in the backend, not `healthie-shared`.** The spec first proposed
  moving `AppConfig` into the shared crate and passing `(db, config)` to the MCP
  router. But the M1b router is `router(db) -> Router` and reads its own env
  (`HEALTHIE_MCP_ALLOWED_HOSTS`) directly — it needs no config. So the signature
  is **preserved unchanged**, the CLI stays in the backend, and `healthie-shared`
  remains a clap-free pure domain library. `TokenKind` is a plain domain enum; the
  backend maps its own clap `--kind` value enum onto it.
- **`/mcp` mounts outside CORS.** The API and `/ingest/hae` run under permissive
  CORS; the MCP router is nested _after_ that layer, because MCP clients are not
  browsers and permissive CORS there would let any web page a user visits drive the
  tools. The ingest bearer gate is route-scoped to `/ingest/hae` alone, so
  `/api/health` is never behind it.

### 8. Ingest returns 204; the report is logged

`POST /ingest/hae` returns `204 No Content` on success. The `IngestReport`
(counts, quarantined names, date range) is logged via tracing — HAE is an
automated poster with nothing to do with a response body, and the quarantine rows
are the durable record of anything unrecognized.

## Consequences

- **Positive:** metrics are queryable typed rows from day one, so M2b's trend and
  recommendation tools have real history to build on without a data migration.
- **Positive:** nothing from a push is ever silently lost — unknown names
  quarantine verbatim, and the exclude list keeps that bucket exceptional rather
  than noisy.
- **Positive:** kinded tokens give ingest and MCP independent blast radius; one
  binary means one thing to deploy, migrate, and back up.
- **Negative / limits:** curation is a standing editorial task — a genuinely useful
  new Apple metric sits in quarantine until someone promotes it; the exclude list
  must be maintained by hand. No per-reading history (last-write-wins collapses a
  day to one row per kind); sleep is stored as stage _totals_, not timed segments
  (deferred to M2b).
- **Enforced by shape, not by a check:** the curated/excluded/unknown split is a
  single `map_hae_name` match — the exclude list and the unknown default are the
  only two ways a name avoids the store, and both are visible in one function, so
  the "quarantine stays exceptional" property is auditable in one place.
