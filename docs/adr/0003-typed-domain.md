# ADR-0003: Typed domain — chrono timestamps and SeaORM enums over glovebox's TEXT-as-String

- **Status:** Accepted
- **Date:** 2026-07-17
- **Related:** healthie-8nb, healthie-rq2, healthie-k18

## Context

`healthie-shared` was scaffolded by copying conventions from the `glovebox-shared`
reference crate. Two of those conventions traded type safety for expedience:

1. **Timestamps and dates were stored as `String`.** Every `*_at` / `*_on` column was
   a `String` in TEXT format, with `clock::now_str()` / `today_str()` producing
   `YYYY-MM-DD HH:MM:SS` / `YYYY-MM-DD`. The briefing assembler then re-parsed those
   strings (`date_part`, `days_between`) to do arithmetic, and every date comparison was
   a lexicographic `&str` compare.
2. **Enumerated columns were validated against `VALID_*` string whitelists.** Status,
   tag, kind, comparison, verdict, origin, and sex were `String` fields guarded by
   hand-written "must be one of …" `BadRequest` checks in the services.

Both push validation to runtime and leave invalid states representable in the type
system. As the system grows toward the M1b MCP boundary — where tool schemas want
precise input types — string-typed domain values are a recurring source of avoidable
error handling.

## Decision

**Deviate from the glovebox convention: make the domain types precise.**

- **chrono types (healthie-8nb).** `*_at` columns are `chrono::DateTime<chrono::Utc>`
  (SeaORM `DateTimeUtc`, migration `timestamp_with_time_zone()`); `*_on` and date fields
  are `chrono::NaiveDate` (SeaORM `Date`, migration `.date()`). `clock::now()` / `today()`
  return typed values. The briefing assembler does real date arithmetic
  (`(today - completed_at.date_naive()).num_days()`) instead of string parsing. The
  `datetime('now')` SQL defaults on `created_at`/`updated_at` are dropped: services always
  `Set` both timestamps through the sqlx `DateTime<Utc>` encoder, and a default-written row
  would use a space-separated text format that sorts before every RFC3339 row we write.
- **SeaORM enums (healthie-rq2).** Each `VALID_*` whitelist becomes a `DeriveActiveEnum`
  living in the entity that owns the column (`ConcernStatus`, `ConcernTag`, `GoalStatus`,
  `GoalComparison`, `ProtocolKind`, `ProtocolVerdict`, `ObservationOrigin`,
  `ObservationKind`, `PlanItemKind`, `OutcomeStatus`, `Sex`). Invalid states are
  unrepresentable, so all 11 consts, `validate_tags`, and every whitelist `BadRequest` are
  deleted. The enums derive `EnumIter`, so `<Enum>::iter()` is the canonical way to
  enumerate the legal values (e.g. for M1b tool schemas) — it replaces the deleted `VALID_*`
  arrays. Note that `Copy` is our addition for ergonomic service parameters; the kammerz
  reference idiom derives `Clone` only.
- **Stricter clippy doc lints (healthie-k18).** With most whitelist `BadRequest`s gone, the
  remaining error surface is small enough to document, so `clippy::missing_errors_doc`,
  `missing_panics_doc`, and `must_use_candidate` are re-enabled and every public service fn
  documents its `# Errors`.

`jiff` was considered and rejected: SeaORM 1.1 only integrates with `chrono` and `time`.

## Consequences

- **Positive:** invalid domain states are unrepresentable; whole classes of runtime
  validation are deleted. Date logic is real arithmetic, not string parsing.
- **Positive:** M1b gets precise input types (typed enums and dates) to derive MCP/schemars
  schemas from, for free.
- **No data migration:** SQLite still stores TEXT (sea-query renders both column types with
  text affinity), and every enum's string value matches the prior data, so DB contents and
  JSON wire format are unchanged. This is greenfield — the initial migration is edited in
  place, nothing is deployed.
- **One documented divergence:** `ObservationOrigin::SelfReported` stores the DB
  `string_value` `self_reported` while keeping the serde wire value `self`. SeaORM 1.1's
  `DeriveActiveEnum` Pascal-cases each `string_value` into an internal marker-enum
  identifier, and `self` collides with the reserved keyword `Self`, which will not compile.
  The DB token is internal (no query filters on `origin`), so the MCP wire contract stays
  byte-identical.
- **Deliberate divergence from glovebox:** this is a conscious departure from the reference
  crate's conventions, documented here so future work does not "fix" it back toward
  TEXT-as-String.
