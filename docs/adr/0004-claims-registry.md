# ADR-0004: Claims-with-confidence registry — provenance and revision, not transcripts

- **Status:** Accepted
- **Date:** 2026-07-18
- **Related:** healthie-26g (M1c baseline intake), ADR-0002 (confidence vocabulary
  - intake robustness), ADR-0003 (typed vocabularies), `healthie-shared/src/entities/claim.rs`,
    `healthie-shared/src/services/claim.rs`. Consumed later by M3's screening rules,
    which read `topic` / `occurred_on`.

## Context

The first (dry-run) checkin confirmed the `checkin` script is deliberately narrow:
it covers "since the last checkin," not a person's whole history. The baseline —
the durable record of Steve's health history — needs its own instrument, one that
digs broadly, survives across many sittings, and records what Steve _claims_ rather
than laundering memory into fact.

ADR-0002 already set the frame: baseline intake records **claims with confidence**
(`verified` / `recalled` / `unknown` / `not-done`), `unknown` is "a task to resolve,
never a nag," `run_baseline_intake` / `record_intake_answers` are reserved intake
verbs, and lightweight `FamilyEvent` context is queryable but not per-person
modeling. What ADR-0002 left open is the _shape_ of the record and how intake
resists the specific LLM failure mode Steve flagged: an off-hand remark quietly
becoming canon. This ADR records those calls.

## Decision

### 1. Claim shape: prose-primary hybrid

One `claims` table. The prose `statement` is the primary record — the claim in
Steve's own terms — with a small set of optional typed fields hung off it:
`category`, `confidence`, `subject`, `topic`, `occurred_on`, `source_quote`,
`concern_id`, timestamps.

- **Why not free-text-only:** M3's screening rules need something queryable. "When
  was the last colonoscopy?" cannot be answered by scanning prose; `topic` and
  `occurred_on` give the rules layer a key and a date to filter on.
- **Why not fully-typed:** health history is too irregular and open-ended to force
  into a fixed schema without either shredding the nuance or exploding into a
  per-category table zoo. Prose keeps the human meaning intact and honest, and the
  LLM reads prose natively. The hybrid records the claim as stated and layers just
  enough structure for the rules that will consume it.

### 2. `subject` carries family history

`subject: Option<String>` — absent means the claim is about Steve; a value
("father") means it is about that relative. Family history is therefore a claim
_about the relative_, stored as such, not folded into Steve's own history.
**Propensity inference stays at conversation time** — "your father's afib raises
your own risk" is Claude's judgment when reasoning, not a derived row in the
registry, which stores only what was claimed. `FamilyEvent` (ADR-0002) remains
reserved for _ongoing_ family context (an illness in progress), distinct from these
historical family claims.

### 3. Sessionless intake — coverage derived from the registry

There are no intake-session tables. `run_baseline_intake` computes per-category
coverage (claim count, unknown count, last touched) from the claims themselves;
`record_intake_answers` appends a batch. **The intake state _is_ the registry.**

A session is an event with a start and an end, and a dropped conversation corrupts
it. Deriving coverage from the claims makes intake **resumable by construction**: a
dropped conversation loses nothing, and the next sitting orients from the same
computed map. The baseline is a _state of completeness_, deepened across many
conversations — not a wizard to be finished in one run.

### 4. Provenance and revision, not transcripts

The failure mode this defends against: an off-hand remark quietly becoming canon.
Claude hears "I think my dad's afib started in his 60s?" and files "father had afib
onset at 62" as settled fact. A full Q&A transcript would preserve the original
evidence — but transcripts are evidence nobody re-reads. So instead:

- `source_quote` captures the verbatim words a claim was distilled from, and it
  **travels with the claim**. Calibration drift stays visible on the record itself,
  where anyone reading the claim also sees what it came from.
- `source_quote` is **immutable by construction**: `UpdateClaim` has no field for
  it, so no revision path can rewrite the evidence. Everything else on a claim is
  revisable via `update_claim` — which is also how an `unknown` resolves once
  records are checked (`unknown → verified`).
- The `baseline_intake` prompt scripts a **read-back** of just-recorded claims
  before they settle, so Steve corrects the calibration in the moment rather than
  discovering a laundered fact later.

### 5. Category vocabulary and what the briefing surfaces

Eleven categories, in the ADR-0003 house style (`DeriveActiveEnum`, kebab-case
serde, `EnumIter`, feature-gated `JsonSchema`): `family-history`, `condition`,
`surgery`, `injury`, `screening`, `medication`, `supplement`, `allergy`,
`mental-health`, `lifestyle`, `general`. `general` is a **quarantine, never a
drop** — anything the vocabulary didn't anticipate lands there rather than being
discarded, the same never-silently-drop discipline ADR-0002 applies to unknown HAE
metric kinds.

The briefing surfaces only `claims_needing_resolution` (confidence = `unknown`) —
"a task to resolve, never a nag" (ADR-0002). Coverage _gaps_ stay out of the
briefing; surfacing what is still untouched is `run_baseline_intake`'s job, on
demand, not a standing reminder.

## Consequences

- **Positive:** intake survives across many sittings and a dropped conversation
  loses nothing — the registry is its own progress state, no session lifecycle to
  corrupt.
- **Positive:** provenance travels with every claim, so the "off-hand remark
  becomes canon" failure mode stays visible and correctable rather than buried in a
  transcript no one revisits.
- **Positive:** M3's screening rules get queryable `topic` / `occurred_on` for
  free, without forcing irregular health history into a rigid schema.
- **Negative / limits:** no dedup or merge of claims — two sittings can record
  overlapping claims, and reconciliation is conversational for now; no per-person
  family modeling (`subject` is a free string, not a family graph); prose
  `statement`s are not validated beyond non-empty.
- **Enforced by shape, not by a check:** `source_quote` immutability holds only
  because the field is absent from `UpdateClaim`. A future edit that adds it would
  silently break the provenance contract — documented here so it is not "fixed" in
  later.
