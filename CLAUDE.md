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
just ci        # full gate: build + test + clippy (workspace, --locked)
just fmt       # cargo +nightly fmt (nightly required for rustfmt.toml options)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings -D clippy::pedantic  # hard gate
```

## Architecture Overview

Personal health system-of-record per `../personal-domain-pattern` (vision:
`docs/superpowers/specs/2026-07-16-healthie-vision-reset-design.md`, being distilled
into `docs/adr/`). Cargo workspace: `healthie-shared` (SeaORM/SQLite domain lib —
entities, migrations, services, briefing assembler) is live; `healthie-mcp` (M1b),
`healthie-backend` + Svelte SPA (M2+) to come. Conventions copied from `../glovebox`;
legacy Python files at root are dead (healthie-45d) — ignore them.

## Conventions & Patterns

- Services: free async fns over `&impl ConnectionTrait`; multi-row writes take
  `ConnectionTrait + TransactionTrait` and wrap all writes in one txn
- All service fns return `DomainResult`; validation lives in services, never in
  handlers/MCP tools; whitelist errors list valid values in the message
- Tests: in-memory migrated `test_support::test_db()` (behind `test-support` feature);
  inline `#[cfg(test)]` per service; date-dependent fns take `today: &str` param
- SeaORM gotcha: never `.save()` with a `Set` PK — it always takes the UPDATE path
  (RecordNotUpdated on first insert); branch `.insert()`/`.update()` explicitly
- SQLite FKs are declared but INERT (no `PRAGMA foreign_keys=ON` yet — healthie-38x);
  services self-enforce referential integrity via `require()` — keep doing so
- Do NOT commit new `docs/superpowers/` specs/plans — decisions become ADRs in
  `docs/adr/` (healthie-19k); active plans stay untracked on disk
- Pending type-tightening (check `bd ready` before relying on current shapes):
  chrono datetimes (healthie-8nb), enum columns replacing string whitelists
  (healthie-rq2), stricter clippy doc lints (healthie-k18)
