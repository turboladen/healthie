# Architecture Decision Records

This directory holds **ADRs** — short, immutable records of a significant decision:
the context that forced it, the choice made, and the consequences. They answer
"why is it this way?" for the next person (usually future-you). ADRs are the
**durable decision record** for this project.

## Why ADRs

A decision's current relevance is explicit in its **Status**: an `Accepted` decision
is live; a reversed one becomes `Superseded by ADR-N`, and the replacement points
back with `Supersedes ADR-M`. You never edit a decision's history — you supersede
it. Grep a topic and the status tells you immediately whether you're looking at
current truth or a retired call.

## Relationship to `docs/superpowers/` design docs

Design and planning happen through the brainstorming / writing-plans workflow, which
produces a spec + implementation plan under `docs/superpowers/`. Those are
**transient working artifacts** — scaffolding useful while a feature is in flight —
**not** a permanent archive, and they are never committed. The durable _decision_
inside a design gets promoted to an ADR here; the spec/plan itself is retired once
the feature ships (it survives in git history if ever needed). The founding
vision-reset spec was retired this way — its decisions were distilled into
[ADR-0002](0002-personal-domain-pattern-rebuild.md).

## When to write one

Write an ADR when a decision is **cross-cutting / architectural** OR **likely to be
revisited or reversed** — a data-model convention, a framework choice, a policy that
spans many files. Not every feature needs one: a refactor whose conventions already
live in code + `CLAUDE.md` does not.

Rule of thumb: if reversing it later would need a "why did we do that?" explanation,
it's an ADR.

## Conventions

- One file per decision: `NNNN-kebab-title.md`, numbered sequentially.
- Format: a status block + **Context / Decision / Consequences** (Nygard style).
- **Immutable once `Accepted`.** To change a decision, add a new ADR that
  `Supersedes` it and flip the old one's status to `Superseded by` — only the status
  line of a superseded ADR is edited, never its body.
- `Status` values: `Accepted`, `Superseded by ADR-N`, `Deprecated`, `Proposed`.
- **Related:** blocks reference bead IDs and durable artifacts (code paths,
  `CLAUDE.md`, PRs), not transient design docs.

## Index

| ADR                                             | Status   | Decision                                                   |
| ----------------------------------------------- | -------- | ---------------------------------------------------------- |
| [0001](0001-record-architecture-decisions.md)   | Accepted | Adopt ADRs as the durable decision record                  |
| [0002](0002-personal-domain-pattern-rebuild.md) | Accepted | Rebuild on the personal-domain pattern: checkin loop first |
