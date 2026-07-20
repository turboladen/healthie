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

## Run the MCP server

```bash
# Provision the bearer token — printed ONCE, store it now.
healthie-mcp token provision

# Serve (defaults: --db-path data/healthie.db, --listen 0.0.0.0:3005).
healthie-mcp serve

# Exposing over Tailscale? rmcp's DNS-rebinding defense rejects unknown Host
# headers — allowlist your hostnames (comma-separated; blank = any port):
HEALTHIE_MCP_ALLOWED_HOSTS=odroid.tailnet.ts.net,dietpi.local:3005 healthie-mcp serve

# Rotate or revoke the token:
healthie-mcp token provision   # rotates; previous token stops working
healthie-mcp token revoke      # all requests 401 until re-provisioned
```

Every request needs `Authorization: Bearer <token>`; the token is stored only as
an argon2id hash and never logged.

## Project docs

- `docs/adr/` — architecture decision records (the durable "why")
- `CLAUDE.md` — agent conventions and build/test commands

## Status

M1a complete: `healthie-shared` domain library (entities, migrations, services,
briefing assembler). M1b complete: `healthie-mcp` — bearer-authed rmcp server
(15 tools, `healthie://briefing` resource, `checkin` prompt) with a binary host
until the M2 backend nests its `router()`. M1c complete: claims-with-confidence
registry + intake tools (`run_baseline_intake`, `record_intake_answers`,
`update_claim`, `get_claims`) and the `baseline_intake` prompt — 19 tools total.
Next: `healthie-backend` + Svelte SPA.
