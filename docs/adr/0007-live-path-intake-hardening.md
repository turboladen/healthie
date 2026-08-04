# ADR-0007: Live-path intake hardening — refuse, don't coerce; bound, don't clamp

- **Status:** Accepted
- **Date:** 2026-08-01
- **Related:** healthie-ei8 (live-path unit coercion), healthie-55h (plausibility
  bounds, both paths), healthie-c47 (whole-row upsert), ADR-0005 (§4 the
  curated/excluded/quarantined trichotomy, §5 the idempotent upsert this
  **clarifies**), ADR-0006 (§6 refuse-rather-than-coerce, §7 the
  quarantine-granularity argument this reuses), ADR-0002 ("never silently
  dropped"), ADR-0003 (typed vocabularies). Code:
  `healthie-shared/src/services/{metrics.rs,units.rs,plausibility.rs,apple_health/}`,
  `healthie-shared/src/entities/quarantined_metric.rs`. Open: healthie-t58 (HAE's
  percent convention), healthie-1ru (quarantine discriminator column).

## Context

ADR-0006 built an `export.xml` importer that refuses rather than coerces: a unit
it cannot convert quarantines instead of being stored as though it had always
been canonical. Building that made visible that the **live** path was laxer in
exactly those ways, and the live path is the one that will run unattended every
day once the odroid deploy lands, with no interactive caller to notice a warning.

Three specific defects:

1. `ingest_hae` emitted a `tracing::warn!` when HAE's declared units disagreed
   with `MetricKind::unit()` and then stored the number anyway. A payload
   declaring `kg` for weight was stored as if it were pounds — a 2.2x error,
   permanent, silent apart from a log line.
2. Neither intake applied any range or sign validation. The first real
   import surfaced a 49.88 h sleep night (verified against the raw XML: a sleep
   app left running for a day and a half) and a 0.000 SpO2 floor across 962 days
   — a reading that is not survivable. healthie-4lf.2 thresholds on exactly
   those columns.
3. `upsert_metric` nulls `min`/`max`/`source` on update, which ADR-0005 §5
   neither states nor forbids, and which nothing tested.

## Decision

### 1. The live path converts, and refuses what it cannot convert

Values reaching `daily_metric` are converted to `MetricKind::unit()` through the
same UCUM-backed `convert_to_canonical` the backfill uses, including the
`min`/`max` bounds, which were previously stored unconverted even when the value
was not. A missing or unconvertible unit **quarantines the point verbatim**
rather than assuming it arrived canonical.

HAE declares `units` on the _metric_, not the point, so the verbatim point alone
cannot support "fix the vocabulary and re-POST" — it does not contain the unit.
Quarantine rows therefore carry `_import.units` and `_import.kinds` beside
`_import.reason`, and `upsert_quarantine` stamps all three itself so a
reason-less row is unconstructible rather than merely discouraged.

### 2. Percent scale belongs to the producer, not to the unit

`convert_to_canonical` moved out of `apple_health/` because its vocabulary
always described two vendors' spellings. But moving it as it stood would have
been a 100x change disguised as a refactor: `PERCENT_SCALE = 100` was calibrated
against `export.xml`, where HealthKit's `%` is a 0-1 fraction, and applying that
to HAE's numbers assumes a convention nothing in this codebase has verified.

So the scale is a named `Producer`'s, and the evidence for each arm sits on that
arm: **verified** for `AppleExportXml` (ADR-0006 §6), explicitly **UNVERIFIED**
for `HealthAutoExport`. Unity is chosen for HAE because it preserves exactly
what the live path did before it converted at all — routing HAE through
conversion cannot, by construction, move a stored number by 100x in either
direction.

healthie-t58 settles it the way healthie-4u7 settled the export side: by reading
the observed span from real data. Two close paths exist, because the first
turned out not to be available on demand — the HAE MCP server
(`.mcp.json`) can be read directly when the device is on the network, and
failing that, `ingest_hae` warns whenever a percent-typed kind arrives at or
below 1.0, which is what a fraction-sending HAE looks like. The live path has no
span report, so without that warning a `0.303` body fat would store silently:
it is comfortably inside every plausibility bound.

### 3. Bounds encode impossibility, not unusualness

`services::plausibility` declares, per `MetricKind`, what a reading can
physically be. A value outside those bounds is **not a measurement**; a value
inside them is stored untouched however strange it looks.

That line is deliberate. Deciding what is _unusual_ needs the real distribution,
which is healthie-4lf.2's job over exactly the data these bounds must not
distort — so the ceilings are generous and the interesting limits are
definitional: 0% oxygen saturation, 0 lb, 0 bpm, more than a day of sleep in a
day.

**Nothing clamps.** A 49.9 h night rounded to 24 h is a fabricated measurement
that every later reader would trust. We do not know how long he slept. The
honest row is no row.

**Per kind, because one rule is provably wrong on this data.** The real import
holds 243 `gait-asymmetry` days and 2 `breathing-disturbances` days reading
exactly 0.0 — a perfectly symmetric gait and a night with no disturbances are
what a _good_ reading looks like — beside 40 `body-fat` and 23 `spo2` days at
0.0 that are impossible. A blanket "positive values only" would have destroyed
245 real rows to catch 63 artifacts, and the unit does not say which kind is
which. `LowerBound::{AtLeast, Above}` makes that distinction the type rather
than a comment.

Two bounds are worth stating as the judgments they are:

- **Sleep stages ≤ 25 h.** This is _not_ a property of the fold, and an earlier
  draft of this decision wrongly claimed it was. Segments are attributed by
  their start hour and never split (`accumulate::sleep_date`), so a single 36 h
  segment lands whole on one date — which is precisely how the real export
  produced a 49.9 h night. Nothing about the construction bounds the window
  above. 25 rather than 24 because a DST fall-back day genuinely holds 25
  wall-clock hours, and 24 would refuse a legitimate longest day.
- **`TimeInBed` ≤ 48 h.** Being in bed is not a claim about consciousness, and
  more than a day continuously in bed is ordinary for a bedridden or
  post-operative day. Past two days a stuck recorder is the better explanation,
  which is what the export's 52.95 h was.

`spo2 > 0` rather than the `>= 50` first proposed: a genuine 60% saturation is a
medical emergency, and discarding a real emergency reading is the one failure
direction that would matter. `> 0` catches all 23 confirmed artifacts and
discards no reading a body can produce.

**What bounds deliberately do not catch: a scale error.** An unscaled 0-1
percentage sits comfortably inside `(0, 100]`. ADR-0006 §6 settled that a range
heuristic gets this wrong — `GaitDoubleSupport` arrives spanning `0.259 ..
0.358`, entirely plausible _as_ a percentage — so the value-span report remains
the tripwire for scale and these bounds neither replace nor weaken it.

### 4. A bad value costs the point; a bad bound costs only the column

Uniform across both intakes and across both non-finite and out-of-range values:

- `value` unusable → **no row**. There is nothing storable in it.
- `min`/`max` unusable → **the row lands with that column empty**. The day's own
  figure is independently sound, and discarding it to punish a sensor glitch in
  the spread throws away good data.

**Known limit, stated rather than implied:** a cleared `min` is
**indistinguishable downstream** from a `Mean`/`Sum`-policy row that legitimately
has none, and from an HAE aggregate that simply arrived without a `Min`.
healthie-4lf.2 cannot tell a partial row from a complete one, and nothing on the
row marks it. §5 records a second limit of the same shape: a night's stages and
its total can come from different pushes.

On the import path the bound applies at **two levels**, for different reasons.
Per _record_, before the fold: excluding an artifact reading is what lets the
day keep its real minimum, since refusing it only at the rollup would clear the
whole column and lose the genuine low with the fake one. Per _rollup_, after it:
sleep has no per-record value to bound, because a night only exists after the
interval union. Note the same per-day table is applied per record, where the
ceilings are nearly vacuous — no single record approaches 200,000 steps — so
what bites at that level is the lower bound, on the kinds where one reading is
directly comparable to a daily figure.

### 5. Live refusals quarantine; import refusals are counted and reported

This asymmetry is deliberate and follows one principle: **preserve what cannot
be recovered elsewhere.**

The live path's POST body is gone the moment the handler returns, so anything
refused is written verbatim to `quarantined_metric` with the reason and the
declared units. The import path's records are still on disk in `export.xml`, so
a refusal is counted per kind with a sample and printed — a widened bound plus a
re-run recovers every one of them. That is ADR-0006 §7's argument for
one-row-per-name, applied to a second question.

Unit problems quarantine on **both** paths; only plausibility refusals differ.

A refused _sleep stage_ costs only that stage, not the point. Each stage is its
own `(kind, date)` row, and a refused one is the same shape as an absent stage
field, which ADR-0005 §3 already defines as skipped-not-zeroed. On 2023-12-29
the total and time-in-bed are impossible while deep, REM and core are ordinary
and informative; refusing the whole point would discard three good rows to
punish two bad ones.

**The cost of that choice, stated:** because refusal is per stage and writes are
per `(kind, date)`, a night can end up **mixed**. If day D already holds a
`sleep-total` of 7.2 from an earlier push, and a re-push carries a 49.877 total
alongside ordinary deep/REM/core, the total keeps the _old_ push's number while
the stages take the new one's — one row's worth of night assembled from two
different accounts, with nothing marking it. healthie-4lf.2 will read them as
one night. The per-stage rule is still right (the alternative discards good
rows), but this is a real consequence rather than a hypothetical one, and it
belongs beside the indistinguishable-cleared-`min` limit in §4.

### 6. A resolved complaint does not outlive its cause

When a `(raw_name, date)` later stores cleanly, its quarantine row is deleted in
the same transaction. Upsert never deletes, so without this every recovery —
widen a bound, add a unit spelling, re-POST — would leave a row behind
complaining about a problem that no longer exists, and ADR-0005 §4's "quarantine
stays exceptional" would quietly stop being true.

A _persistent_ cause still accrues one row per day. That is the complaint
working, not litter.

Outcomes are accumulated per `(raw_name, date)` and resolved **after** the point
loop rather than inside it. `quarantined_metric` is keyed per `(name, date)`
while refusals are per point, so writing as we go would let the order of the
`data` array decide whether a day ends up complained about or swept clean.

Where two points on one key disagree, **severity** decides which is preserved:
a point that stored nothing outranks one that only lost a bound, because the
first one's reading exists nowhere else while the second's value is already in
`daily_metric`. **A same-severity tie is still arrival order** — of two points
that both stored nothing, the first is kept. That one is not resolvable here:
one row per key means one of two equally-total losses is unrecoverable whatever
rule is chosen, and the fix is a finer key (healthie-1ru), not a better
tie-break.

### 7. A write is one intake's whole account of a day

`upsert_metric` replaces the whole row — `min`, `max` and `source` included — so
a scalar push after an aggregate clears the spread. **This clarifies ADR-0005
§5, which describes the row upserting and is silent about columns; it does not
supersede it, because the observable behavior is unchanged.**

Kept rather than coalesced, because coalescing produces a **chimera row**:
today's `value` from one intake beside a spread computed months ago by another,
under a `source` naming only one of them. Not hypothetical — the `Mean` and
`Sum` policies resolve to `(value, None, None)` and the backfill writes those
onto dates where a live HAE aggregate may already sit. healthie-4lf.2 thresholds
on `min`/`max`, and a stale bound under a fresh value is a silent lie with
nothing on the row to mark it.

## Consequences

- **Positive:** the two intakes now agree about units, and the identical
  mismatch is no longer fatal-per-record on one path and silent on the other
  while both write the same column.
- **Positive:** every refusal is durable and recoverable, with one bound worth
  stating precisely. The live path holds the point verbatim with the unit and
  reason, and the import path's records are still in the file — so the
  guarantee is **one preserved point per `(raw_name, date)`**, not one per
  refusal. When several points for one metric on one day are all refused, the
  single row keeps the most severe and the rest are discarded. HAE sends one
  point per metric per day, so this is a corner rather than the common case,
  but "nothing is lost" would be the wrong thing to remember.
- **Positive:** the risk is instrumented rather than assumed. Ingest logs at
  `warn` when a push refused anything, so a metric that starts failing on every
  push is visible on the first unattended day rather than discovered in a trend
  months later.
- **Measured, not asserted:** re-importing the real 3.2 GB export under
  these rules wrote 50,812 rows against 50,877 before — **65 fewer, 0.13%**, and
  every one a confirmed artifact: 40 `body-fat` and 23 `spo2` days reading
  exactly 0.0, the 49.877 h night, and the 52.953 h time-in-bed. All 245
  legitimate zeros survived. `spo2`'s stored floor moved from 0.000 to 90.667
  and its `min` column from 0.000 to 79.0 — a real desaturation low, kept.
  Daily means shifted where per-record exclusion changed the average (`spo2`
  +2.278, `body-fat` +1.054), which is the intended effect and not a
  regression. Every value in the plausibility regression test is one of those
  measured extremes, so a future tightening has to argue with real data.
- **The record-level check did all the work.** `bounds_cleared` was **zero** on
  the real import: excluding artifact readings before the fold meant no rollup
  ever had to drop a column, so days kept a genuine `min` instead of an empty
  one. The row-level bound clearing exists for the live path, where readings
  arrive pre-aggregated and there is nothing finer to exclude.
- **Negative / limits:** a cleared `min` is indistinguishable from a row that
  never had one (§4). Bounds cannot detect a scale error (§3). HAE's percent
  convention remains unverified (healthie-t58), and until it is settled the live
  and import paths can disagree by 100x on the four percent kinds.
- **Nothing here cleans up rows already stored,** and the available remedy is
  narrower than it looks. Upsert never deletes, so a row an earlier run wrote
  from a value these bounds now refuse simply stays, holding a normal-looking
  number. On the **import** path a re-run surfaces it as a `stale_row` and
  `--replace-range` deletes it — but only for kinds that run still produces, and
  only inside that kind's own surviving date range (`find_stale_rows`). On the
  **live** path there is no equivalent sweep at all: `clear_quarantine` retires
  resolved _complaints_, not superseded `daily_metric` rows. A day whose value
  is now refused keeps whatever was stored for it, with nothing on the row to
  mark it. Cleaning up the 65 rows already in an imported store is therefore a
  deliberate follow-up, not something this change does.
- **Enforced by shape, not by a check:** a new `MetricKind` cannot ship without
  bounds, because the match is exhaustive with no wildcard — the same mechanism
  that already forces it to declare a unit and an aggregation policy. And a
  quarantine row cannot be written without a reason, because `upsert_quarantine`
  takes one.
