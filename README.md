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

## Run the backend

`healthie-backend` is the single deployed binary: the REST API, `POST
/ingest/hae` (Health Auto Export intake), and the MCP router mounted at `/mcp`.

```bash
# Provision a bearer token — printed ONCE, store it now. --kind is mcp | ingest.
healthie-backend token --kind mcp provision
healthie-backend token --kind ingest provision

# Serve (defaults: --db-path data/healthie.db, --listen 0.0.0.0:3005).
healthie-backend serve

# Exposing over Tailscale? rmcp's DNS-rebinding defense rejects unknown Host
# headers — allowlist your hostnames (comma-separated; blank = any port):
HEALTHIE_MCP_ALLOWED_HOSTS=odroid.tailnet.ts.net,dietpi.local:3005 healthie-backend serve

# Rotate or revoke a token (per kind):
healthie-backend token --kind ingest provision   # rotates; previous token stops working
healthie-backend token --kind ingest revoke      # that kind's requests 401 until re-provisioned
```

The MCP surface (`/mcp`) and `/ingest/hae` each require `Authorization: Bearer
<token>` of the matching kind; tokens are stored only as argon2id hashes and
never logged. Point the Health Auto Export automation's REST target at
`http(s)://<host>:3005/ingest/hae` with the ingest token. `GET /api/health`
needs no token.

## Deployment

healthie runs as a systemd service on the household ODroid N2+ (DietPi,
aarch64). One-time per box, then once per release:

```bash
cp deploy/.env.example deploy/.env
just bootstrap dietpi@dietpi.home     # user, dirs, .env, unit — does not start it
just deploy    dietpi@dietpi.home     # snapshot, upload, atomic swap, health gate
just token     dietpi@dietpi.home ingest   # bearer tokens; printed ONCE
```

`deploy` refuses a dirty tree and waits for `/api/health` to report the exact
commit it just built — a healthy answer from the previous binary does not pass.
`just rollback <host>` restores it; `just status` / `just logs` inspect it.

The `http://healthie.local` name is configured in the separate `home-proxy`
repo (paste `deploy/healthie.caddyfile` there); healthie's deploy warns until
that is done but does not depend on it.

Full runbook: [`docs/operations.md`](docs/operations.md). Rationale:
[ADR-0008](docs/adr/0008-deploy-posture.md).

## Project docs

- `docs/adr/` — architecture decision records (the durable "why")
- `docs/operations.md` — deployment and on-box runbook
- `CLAUDE.md` — agent conventions and build/test commands

## Status

M1a complete: `healthie-shared` domain library (entities, migrations, services,
briefing assembler). M1b complete: `healthie-mcp` — bearer-authed rmcp server
(15 tools, `healthie://briefing` resource, `checkin` prompt) with a binary host
until the M2 backend nests its `router()`. M1c complete: claims-with-confidence
registry + intake tools (`run_baseline_intake`, `record_intake_answers`,
`update_claim`, `get_claims`) and the `baseline_intake` prompt — 19 tools total.
M2 complete: `healthie-backend` is the single deployed binary — `GET
/api/health`, bearer-authed `POST /ingest/hae` into a curated `daily_metric`
store (unknown metrics quarantined), and the MCP `router()` nested at `/mcp`;
`healthie-mcp` is now a library (ADR-0005). Next: Svelte SPA + M2b metric
trends/recommendations.
