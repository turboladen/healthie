# ADR-0006: Apple Health `export.xml` backfill — reconstructing the daily rollup

- **Status:** Accepted
- **Date:** 2026-07-31
- **Related:** healthie-4lf.1 (M2 Apple Health importer), ADR-0005 (the
  `daily_metric` shape, the curated/excluded/quarantined trichotomy, and the
  local-calendar-day upsert this path also obeys), ADR-0002 (dependency policy,
  "never silently dropped"), ADR-0003 (typed vocabularies). Code:
  `healthie-shared/src/services/apple_health/`,
  `healthie-backend/src/apple_health.rs`. Deferred: healthie-1ru (quarantine
  discriminator column), healthie-4lf.2 (anomaly windows over this data).

## Context

ADR-0005 built the live intake: Health Auto Export POSTs JSON on a schedule and
`ingest_hae` lands it. That leaves the store empty of everything that happened
before the backend existed. Steve has years of Apple Health history sitting in a
`export.xml`, and the real baseline intake (healthie-sj9) plus M2b's trend layer
(healthie-bx9) are far more useful starting from that history than from a blank
database.

The two intakes are **not symmetric**, and that asymmetry is the whole design
problem:

- HAE posts **pre-aggregated daily** points — a `qty`, or an `Avg`/`Min`/`Max`
  triple. Apple's app did the daily rollup for us.
- `export.xml` is raw **per-reading** `<Record>` elements — millions of them,
  many per day per type, each with its own unit and source device.

So the backfill has to **reconstruct** the rollup HAE gets for free. Doing that
wrong is silent and permanent: a bad policy produces plausible-looking numbers
that quietly poison every trend computed later. This ADR records the calls that
were not already settled by ADR-0005.

## Decision

### 1. A shared name table, not two parallel mappings

A curated `export.xml` name must resolve to the same `MetricKind` as its HAE
counterpart, or the same physical reading lands on a different kind depending on
how it arrived. `HK_METRICS` is therefore a single table carrying **both**
spellings alongside the kind, and a test walks it against `map_hae_name`. There
is no way to add one spelling without the other.

A misspelled identifier is benign-and-loud rather than silently wrong: an
unrecognized name falls through to quarantine and appears in the import report,
instead of landing data on the wrong kind.

### 2. Per-kind daily aggregation policy

Each `MetricKind` declares how a day's readings collapse (`daily_agg`). The
standing preference is **keep the spread, discard nothing** — `min`/`max`
already exist on the row, so preserving the day's range is free.

- `Sum` — Steps, ActiveEnergy, ExerciseMinutes, StandMinutes, WalkingDistance
- `AvgMinMax` — HeartRate, Spo2, RespiratoryRate, Hrv, Weight, BodyFat,
  CardioRecovery, BreathingDisturbances
- `Mean` — WalkingSpeed, GaitAsymmetry, GaitDoubleSupport, RestingHeartRate,
  StepLength
- `Max` — Vo2Max, chosen knowing it ratchets upward and will **not** surface a
  genuine decline; Apple's per-workout estimate is noisy enough that the daily
  best is the more trustworthy figure. Revisit here first if declines must show.

Weight is `AvgMinMax` rather than last-reading-wins so a stray evening weigh-in
cannot become the day's official weight; BreathingDisturbances is `AvgMinMax` so
`max` holds the worst night while `value` keeps the typical-night baseline that
healthie-4lf.2 needs to judge whether a bad night is unusual.

The match is exhaustive with no wildcard: a new `MetricKind` fails to compile
until its policy is decided. It returns `Option`, not a panic — sleep kinds
return `None` because they fold from segments, and an `unreachable!()` there
would be a production panic reachable by any future caller iterating
`MetricKind::iter()`.

### 3. Sum-kind values are a **lower bound**

Apple's export retains every device's account of the same day: an iPhone and a
Watch both counted the same steps, and the Health app de-duplicates by source
priority only at query time. Summing the raw records roughly **doubles** a step
count.

So `Sum` kinds total **per source** and keep the largest single source. This is
imperfect in a specific, documented way: two devices that each captured a
disjoint half-day yield only the larger half. "Sometimes low" beats
"systematically double", but readers — healthie-4lf.2 especially — must treat
these as a floor, not a measurement.

**Why not a fixed device priority (Watch > iPhone):** `sourceName` is free-form
text the user can rename, so a priority list means substring-matching on
user-controlled data. That reintroduces the rename fragility it was meant to fix
and adds a second failure mode (a rename silently demotes the real device).
Max-of-sources is order-independent and pattern-free. The report prints, per Sum
kind, how many days had several sources and the summed-to-kept ratio, so the
magnitude of what this discards is visible rather than assumed — evidence to
revisit on.

### 4. Sleep: an 18:00 sleep-day boundary, and interval **union** not sum

A night spans midnight, so some rule must place it on a calendar date. Segments
are attributed by their **start**, whole and never split, with anything starting
at or after **18:00 local** counted toward the following day.

- Attributing by start with no shift splits every ordinary night across two rows.
- Attributing by _end_ still strands a pre-midnight `Awake` segment on the
  previous day.
- A flat 12-hour shift groups nights correctly but pushes every afternoon nap
  onto the next day.
- 18:00 is Apple's own 6PM–6PM sleep day, so a backfilled row agrees with what
  the Health app displays. Accepted edge: a segment starting 17:50 and ending
  06:00 lands on the earlier day.

Segments accumulate as **coalesced intervals**, not summed durations. Apple's
export contains overlapping records from multiple sources for the same night
(a Watch and a sleep app both recording), and summing durations silently doubles
it. Union is identical to summation when nothing overlaps and correct when it
does.

`SleepTotal` is **derived** as the union of all asleep-class segments — Apple
emits no total. Consequently **`SleepTotal` can be less than the sum of its
stages** when stages from different sources overlap in time. This is correct but
breaks an invariant a reader would naturally assume, so it is stated here and
pinned by a test.

Both undifferentiated spellings are handled: pre-iOS-16 `…Asleep` and iOS-16+
`…AsleepUnspecified` feed the total with no stage row, because a decade of
history contains years of each.

### 5. Unit conversion refuses rather than coerces

`export.xml` stamps each record with the unit it was recorded in, which varies
by locale and device. `daily_metric` has no unit column, so a value must be
converted to `MetricKind::unit()` before storage — and a value that **cannot**
be converted is quarantined verbatim, never coerced. Writing `78` kg into a
column that means pounds produces a plausible number no later reader can detect
as wrong.

Apple's `Cal` (the kilocalorie) is resolved before case folding, because a
lowercase `cal` is conventionally the small calorie — a 1000x difference. The
ambiguous spelling is refused rather than guessed at.

**The residual risk this cannot solve:** whether Apple exports percent-typed
quantities (`OxygenSaturation`, `BodyFatPercentage`, the two gait percentages)
as `0.97` or `97` is undocumented, and we have no real export to check against.
Rather than encode a guess or a heuristic, the import report prints the observed
**value span per kind**: a spo2 column reading `0.91 .. 0.99 %` instead of
`91 .. 99 %` is obvious on sight, on run one, before anything downstream consumes
it. The fix would be one arm in `units.rs` plus an idempotent re-run.

### 6. Quarantine is one row per name — narrowing ADR-0005 §4 for this path only

ADR-0005 §4 quarantines per `(raw_name, date)`. HAE pushes one day at a time, so
that is naturally bounded. A decade-wide backfill sees ~200 uncurated Apple types
across ~4,500 days, which would write **~800k quarantine rows** — database bloat
with no added discovery value.

So this path keeps **one row per `raw_name`** (the first record seen), with
`raw_point._import.records_seen` carrying the total and every name listed in the
report. ADR-0005 §4's purpose — "the raw points are still on disk to backfill
from" — is satisfied _more_ strongly here than on the live path, because the raw
points literally are still on disk in `export.xml`, and re-running after
promoting a name recovers everything.

The live HAE path is unchanged. `quarantined_metric` now holds two vocabularies
(HAE `snake_case`, Apple `HK…`) with no column to distinguish them; the `HK`
prefix is the **interim** discriminator, sufficient because it can never match an
HAE name. **healthie-1ru** tracks the real column.

Re-running after promoting a name also sweeps the stale quarantine row, since
upsert never deletes. That sweep is scoped to rows quarantined **for their
name** (`unknown-type`, `unknown-sleep-stage`); a curated metric quarantined
over an unconvertible unit describes a live data problem that promoting the name
did nothing about, and must survive.

One-row-per-name is enforced across runs, not merely within one. The retained
date is whichever record a run saw first, so importing a second export that
reaches further back would otherwise land a _second_ row for the same name under
a different `(raw_name, date)` key; rows for other dates are dropped as the
sample is written.

### 7. Last-write-wins, but the overlap is reported first

Writes obey ADR-0005 §5 — `(kind, date)` upsert, last-write-wins — so a backfill
overwrites rows a live HAE push already landed. That is the stated contract and
this path keeps it, with no date-fencing flag.

**Re-running is idempotent for value changes, not for key changes**, and the
distinction matters more than it first appears. A corrected unit or aggregation
rewrites the same `(kind, date)` rows, so the old figures are fully replaced. A
corrected **sleep-day boundary** — precisely the fix the day-shift check
prescribes — moves nights onto different dates, and a re-mapped metric name moves
rows to a different kind. Upsert never deletes, so at the trailing edge of every
contiguous run of recorded nights the old row is never rewritten and survives
holding a known-wrong value on a day that now belongs to a different night or to
none. Over a decade with realistic gaps (pre-Watch years, dead batteries,
travel) that is hundreds of orphans per sleep kind, all with entirely normal
values and a high `rows_overwritten` count to hide behind.

So after writing, the import queries each kind it produced across **that kind's
own** date range, subtracts what it wrote, and reports the remainder as stale —
and `--replace-range` deletes them. Reporting without a remedy would be the same
trap as a false alarm: a warning nobody can act on. Deleting is opt-in because
deleting real data is the operator's decision, not a silent consequence of
re-running an import.

The scoping is per kind, not one range shared across the import, because kinds
cover wildly different spans: a decade of heart rate beside two weigh-ins would
stretch the weight window across the whole decade and sweep in every weigh-in
the live push landed in between.

Even per kind this is a **heuristic, and the report says so**. `daily_metric`
records no provenance (healthie-1ru), so a row this import did not write is
equally consistent with an earlier import misplacing it and with the live HAE
push covering a day this export does not. The warning names both readings rather
than asserting the first, because the remedy it offers is deletion.

But those overlapping rows are the **only** cross-check the two intakes ever
get, and overwriting destroys them. So before writing, the import reports per
kind how many rows it is about to replace and how far its reconstruction
diverges from them (mean and max absolute difference, with the date of the
worst).

Sleep gets a dedicated check, because a day-shifted `SleepTotal` is invisible to
every other guard — a shifted row's value span looks perfectly normal. Each
reconstructed night is compared against existing rows one day either side; if a
neighbour fits decisively better, the run says so and names the constant to
change. It is deliberately conservative — ties resolve to same-day, and a
neighbour must win both proportionally and absolutely — because the warning tells
the operator to re-import a decade of history.

**The check is only meaningful on the first import into a store holding live HAE
rows, and it says so rather than pretending otherwise.** Writes are
last-write-wins, so from the second run onward the rows it would compare against
are its _own previous output_. That is not merely uninformative, it inverts:
after a correct boundary fix, run 2 finds its nights bit-identical to run 1's
rows one day over and reports a mismatch in the opposite direction, so the
fix-and-re-run loop the report prescribes would oscillate forever with each run
confidently contradicting the last. Self-comparison is therefore detected — a
preponderance of _exactly zero_ differences, which two independent computations
of a night never produce — and reported as "not independently verified", never as
agreement. A false green light here would be worse than no check at all, because
it retires the one question this importer cannot otherwise answer.

The three means also share one denominator (only nights with a stored row at
`D−1`, `D` and `D+1` are counted). Means taken over different day sets are not
comparable to each other: a single sparse stored row would otherwise yield a
confident verdict conjured out of nothing.

### 8. Streaming parse, and where the memory actually goes

`export.xml` is routinely multi-gigabyte, so the document is never materialized:
a `BufRead` feeds quick-xml's pull reader and the event buffer is cleared every
iteration (quick-xml _appends_ into the caller's buffer — without the clear it
grows to the size of the file). `<Record>` is handled in both its shapes,
`Event::Empty` and `Event::Start` when it carries `<MetadataEntry/>` children.

Retention, stated honestly:

- Quantities are `O(kinds × days)` — one small accumulator per `(kind, date)`,
  never the readings.
- Sleep is `O(stages × nights × disjoint periods per night)`, because intervals
  coalesce on insert. It does **not** grow with record count or with the number
  of recording devices. It is **not** a hard bound: a source writing alternating
  one-minute segments would push a night from ~10–40 disjoint periods to ~500.
  A per-night threshold makes that case loud rather than letting it quietly
  consume memory.

Because readings collapse _before_ any database write, the write volume is
per `(kind, date)` — roughly 100k rows for a decade, rather than one write per
reading. That is two statements per row inside a single write transaction. **No
runtime is claimed**: this has not been measured against a real export, and the
transaction is held open across all of them. Bulk `ON CONFLICT` upsert
(healthie-zp8) is therefore not needed on _volume_ grounds, and reusing
`ingest_hae`'s row-at-a-time helper keeps exactly one last-write-wins
implementation in the codebase — but if the write phase proves slow on the
odroid, healthie-zp8 is the fix.

### 9. `quick-xml`, and layering

`quick-xml` 0.41 pinned per ADR-0002's dependency policy and verified against the
crate source rather than from memory. Its only mandatory dependency is `memchr`
— pure Rust, no build script — which matters because the deploy target is an
aarch64 odroid and the cross-compile check is CI-only.

Per ADR-0002 all logic lives in `healthie-shared`; the backend owns only the
`import-apple-health` subcommand and the report's presentation. The parse is
exposed as a **synchronous, database-free** `parse_export_xml` returning an
opaque `ParsedExport`, with a separate async `persist_import`. That keeps
`healthie-shared` free of a tokio dependency (the ADR-0005 §7 precedent that kept
clap out of it) while letting the backend run the minutes-long parse on
`spawn_blocking` instead of stalling a runtime worker.

## Consequences

- **Positive:** the store starts with years of real history, so baseline intake
  and the trend layer have something to reason over immediately, and re-running
  after any mapping fix is idempotent and cheap.
- **Positive:** every assumption that could not be verified without a real export
  — percent scaling, the sleep-day boundary, multi-source de-duplication — is
  reported as a number on run one rather than encoded as a silent guess.
- **Negative / limits:** Sum-kind values are a lower bound, not a measurement.
  `SleepTotal` may be less than the sum of its stages. The backfill overwrites
  overlapping live rows. Sleep-day attribution is a judgment call that the
  day-shift check can only evaluate once live rows exist.
- **Enforced by shape, not by a check:** the two intakes cannot disagree about a
  metric's kind, because both spellings live in one table row that a test walks;
  and a new `MetricKind` cannot ship without an aggregation policy, because the
  match is exhaustive.
