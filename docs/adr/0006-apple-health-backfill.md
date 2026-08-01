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

The HAE spelling is `Option`, and `None` means that intake has **no 1:1
counterpart** — not that one is missing. HAE models blood pressure as a single
`blood_pressure` metric carrying `systolic` and `diastolic` fields per data
point (verified against its published JSON format), the same 1-to-many shape as
`sleep_analysis`, so no HAE _name_ resolves to either blood-pressure kind.
Inventing `blood_pressure_systolic` would have made the agreement test green
while guaranteeing nothing, since HAE never sends it. The test asserts the
absence positively: if an HAE name is ever mapped to a blood-pressure kind, it
fails until the table is corrected.

### 2. Blood pressure is a pair the row shape cannot express

`daily_metric` is flat `(kind, date, value)`, so systolic and diastolic are
stored as **two independent rows** and re-paired by date at read time. Nothing
records that a given systolic and diastolic were one cuff reading, and at daily
granularity — where several readings already collapse into a mean plus a range
— that association is not recoverable. Acceptable for trend use; readers must
not assume the pairing is modeled. Exploding HAE's `blood_pressure` the way
`sleep_analysis` is exploded would give the live path the same two kinds, and is
the natural follow-up.

### 3. Per-kind daily aggregation policy

Each `MetricKind` declares how a day's readings collapse (`daily_agg`). The
standing preference is **keep the spread, discard nothing** — `min`/`max`
already exist on the row, so preserving the day's range is free.

- `Sum` — Steps, ActiveEnergy, ExerciseMinutes, StandMinutes, WalkingDistance,
  FlightsClimbed
- `AvgMinMax` — HeartRate, Spo2, RespiratoryRate, Hrv, Weight, BodyFat,
  CardioRecovery, BreathingDisturbances, BloodPressureSystolic,
  BloodPressureDiastolic
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

### 4. Sum-kind values are a **lower bound**

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

### 5. Sleep: an 18:00 sleep-day boundary, and interval **union** not sum

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

### 6. Unit conversion refuses rather than coerces

`export.xml` stamps each record with the unit it was recorded in, which varies
by locale and device. `daily_metric` has no unit column, so a value must be
converted to `MetricKind::unit()` before storage — and a value that **cannot**
be converted is quarantined verbatim, never coerced. Writing `78` kg into a
column that means pounds produces a plausible number no later reader can detect
as wrong.

**UCUM is the canonical unit vocabulary** (`ucum-units`), which closes bead
healthie-c4x's trigger: "revisit when unit conversions become real". The
arithmetic comes from the standard's own machine-readable `ucum-essence.xml`,
parsed at build time into generated tables, so no physical constant in this
codebase is transcribed by hand. One runtime dependency (`thiserror`, already
present); the XML parsing is build-time only, keeping the aarch64 cross-compile
target unaffected.

The first implementation hand-derived roughly twenty constants, and replacing
them was not a formality — it was checked. Two other units crates were measured
against that table first and **both were less accurate**: one truncates the
international pound to seven significant figures, the other hardcodes 1609 m per
mile in its speed module (while getting it right in its length module) and uses
the International Steam Table calorie where food energy wants the thermochemical
one. Both were declined on that evidence. UCUM agrees with the hand-derived
table on all twenty conversions — thirteen bit-exact, seven differing only in
the last ULP — and on the one case where they differed measurably (100 kg in
pounds), **UCUM is the correct one**: the hand-derived factor had rounded twice,
once on the factor and again on the multiply, landing one ULP high.

What UCUM deliberately does **not** own is the vocabulary. Apple and HAE write
`mL/min·kg`, `lbs`, `mph` and `mmHg` — the last of which is not valid UCUM at
all (the code is `mm[Hg]`). Mapping those spellings onto codes stays ours,
because it describes two vendors' habits rather than physics, and the canonical
side is derived through that same map so a unit and its code cannot drift apart.

Apple's `Cal` (the kilocalorie) is resolved before case folding, because a
lowercase `cal` is conventionally the small calorie — a 1000x difference. The
ambiguous spelling is refused rather than guessed at.

Having real dimensions also sharpens the refusal: UCUM answers with an error
rather than a wrong number when the two codes are dimensionally incomparable, so
`kg` on a distance metric is rejected on its dimensions rather than merely for
being unmatched.

**Percent is the one unit where matching strings mean different scales.** Apple
writes `%` for HealthKit's `HKUnit.percent()`, which is a **0-1 fraction**;
canonical `%` here is 0-100. Every percent-typed reading is therefore multiplied
by 100 on the way in.

This was not guessed. The importer was built against synthetic fixtures with no
real export available, so rather than assume a scale it deliberately applied no
conversion and printed the observed **value span per kind** instead — turning an
undocumented assumption into a number the operator would read on the first run.

That run happened before this work merged: 7,649,954 records over 2011-02-17 to
2026-07-31, which returned `BodyFat 0.000 .. 0.303`, `Spo2 0.000 .. 0.985`,
`GaitAsymmetry 0.000 .. 0.900` and `GaitDoubleSupport 0.259 .. 0.358` — a 0-1
fraction on every percent-typed kind, answering the question outright. The
conversion above was added in response, and the span report now doubles as the
regression check: a correctly scaled percent kind no longer trips the warning.

Deferring the decision was right rather than merely cautious, because a range
heuristic would have gotten `GaitDoubleSupport` wrong. Its raw
`0.259 .. 0.358` is entirely plausible _as_ a percentage, so an "is the max
below 1.0?" rule applied per kind at import time would have left it 100x low
while correctly fixing blood oxygen — exactly the plausible-looking,
undetectable error this ADR opens by warning about.

The span report remains, now as a tripwire against a future source that does not
scale the same way: a spo2 column reading `0.91 .. 0.99 %` instead of
`91 .. 99 %` is obvious on sight, on run one, before anything downstream consumes
it. The fix would be one arm in `units.rs` plus an idempotent re-run.

### 7. Quarantine is one row per name — narrowing ADR-0005 §4 for this path only

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

### 8. Last-write-wins, but the overlap is reported first

Writes obey ADR-0005 §5 — `(kind, date)` upsert, last-write-wins — so a backfill
overwrites rows a live HAE push already landed. That is the stated contract and
this path keeps it, with no date-fencing flag.

**"Idempotent" is three claims, and only two of them hold.** Re-running the same
import converges, and a **value**-changing fix (a corrected unit or aggregation)
converges, because both rewrite the same `(kind, date)` keys.

Neither is **non-destructive**, which is the claim a reader is most likely to
infer and the one that is false. Last-write-wins overwrites whatever occupied
that key — including rows the live HAE push landed — and that is not
recoverable. Once the backend is deployed and HAE has been ingesting for months,
re-importing a corrected export clobbers good live rows on every overlapping
date, with `--replace-range` nowhere in sight. So the command surveys
`daily_metric` and warns **before parsing**, while aborting still costs nothing;
it stays silent on an empty store, because a warning that always fires is one
nobody reads.

A **key**-changing fix is the third case and fails even the convergence claim. A
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
neighbor fits decisively better, the run says so and names the constant to
change. It is deliberately conservative — ties resolve to same-day, and a
neighbor must win both proportionally and absolutely — because the warning tells
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
of a night essentially never produce — and reported as "not independently
verified", never as agreement. A false green light here would be worse than no
check at all, because it retires the one question this importer cannot otherwise
answer.

This is a heuristic, not a proof: live rows that happened to carry this import's
nights one day over, with coinciding values, would trip it and suppress a real
mismatch. That needs bit-identity across a majority of days and is not achievable
against real HAE figures, and the failure direction is the safe one — it
withholds a verdict rather than issuing a false green light.

**Anything destructive additionally requires a complete file.** A transfer of a
multi-gigabyte export interrupted at a record boundary parses without error and
simply stops early, so `--replace-range` is refused when EOF arrives with the
root element still open: rows "missing" from a partial file are mostly rows the
file does not reach. Importing what a truncated file holds is still allowed, and
the report says the file was truncated.

The three means also share one denominator (only nights with a stored row at
`D−1`, `D` and `D+1` are counted). Means taken over different day sets are not
comparable to each other: a single sparse stored row would otherwise yield a
confident verdict conjured out of nothing.

### 9. Streaming parse, and where the memory actually goes

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

### 10. `quick-xml`, and layering

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
