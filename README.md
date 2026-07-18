# healthie

Personal health system-of-record with an AI checkin loop.

## Tech stack

- **Rust** — axum, SeaORM, SQLite (single deployable binary)
- **rmcp** — MCP server scripting the checkin conversation
- **Svelte 5** SPA (planned)

## Development

```bash
just ci                  # full gate: build + test + clippy
cargo test --workspace
```

## Project docs

- `docs/adr/` — architecture decision records (the durable "why")
- `CLAUDE.md` — agent conventions and build/test commands

## Status

M1a complete: `healthie-shared` domain library (entities, migrations, services,
briefing assembler). Next: `healthie-mcp` server.
