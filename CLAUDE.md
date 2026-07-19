# Project Instructions for AI Agents

This file provides instructions and context for AI coding agents working on this project.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->


## Build & Test

```bash
just ci        # full gate: fmt-check + ci-backend (build + test + clippy, workspace, --locked)
just fmt       # dprint fmt + cargo +nightly fmt (nightly required for rustfmt.toml options)
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings -D clippy::pedantic  # hard gate
```

## Architecture Overview

Personal health system-of-record per `../personal-domain-pattern`; the durable vision
and decisions live in `docs/adr/` (start with ADR-0002, deviations in ADR-0003+).
Cargo workspace: `healthie-shared` (SeaORM/SQLite domain lib — entities, migrations,
services, briefing assembler, claims registry) is live; `healthie-mcp` (M1b/M1c) is
live — an rmcp stateless streamable-HTTP server exposing 19 tools (incl. the M1c
claims-with-confidence intake — `run_baseline_intake` / `record_intake_answers` /
`update_claim` / `get_claims`, ADR-0004), the `healthie://briefing` resource, and
the `checkin` + `baseline_intake` prompts, gated by bearer-token auth (singleton
`mcp_token` service). It ships as a library `router()` (M2's `healthie-backend`
will `nest_service` it) plus a binary (`healthie-mcp serve|token provision|revoke`)
that hosts it until the backend exists. `healthie-backend` + Svelte SPA (M2+) to
come. Conventions copied from `../glovebox` except where an ADR says otherwise.

## Conventions & Patterns

- Services: free async fns over `&impl ConnectionTrait`; multi-row writes take
  `ConnectionTrait + TransactionTrait` and wrap all writes in one txn
- All service fns return `DomainResult`; validation lives in services, never in
  handlers/MCP tools; every public service fn documents its errors (`# Errors`)
- Typed domain (ADR-0003): timestamps are `chrono::DateTime<Utc>`, dates are
  `NaiveDate` — never write string timestamps or SQL datetime defaults; closed
  vocabularies are `DeriveActiveEnum` enums (kebab-case serde), enumerate via
  `::iter()` (EnumIter), never re-introduce `VALID_*` string whitelists
- Tests: in-memory migrated `test_support::test_db()` (behind `test-support` feature);
  inline `#[cfg(test)]` per service; date-dependent fns take `today: NaiveDate`;
  typed seed helpers `test_support::{date, datetime}`
- SeaORM gotcha: never `.save()` with a `Set` PK — it always takes the UPDATE path
  (RecordNotUpdated on first insert); branch `.insert()`/`.update()` explicitly
- SQLite FKs are ENFORCED: `PRAGMA foreign_keys=ON` is explicit on the binary's
  connection (healthie-mcp main.rs) and in `test_db()` (healthie-38x); services
  still self-enforce referential integrity via `require()` as the backstop for
  actionable errors — keep doing so
- Do NOT commit new `docs/superpowers/` specs/plans — decisions become ADRs in
  `docs/adr/` (immutable once accepted; supersede, don't edit); active plans stay
  untracked on disk
