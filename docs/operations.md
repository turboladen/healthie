# Operations — healthie on the ODroid

`dietpi@dietpi.home` (192.168.10.4), aarch64, systemd 252, passwordless sudo.
healthie is :3005, alongside fewd :3000, chorez :3001, kammerz :3002, behind the
shared Caddy on :80. Rationale: [ADR-0008](adr/0008-deploy-posture.md).

## Once per dev machine

```bash
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
rustup target add aarch64-unknown-linux-gnu
```

## Once per box

```bash
cp deploy/.env.example deploy/.env
just bootstrap dietpi@dietpi.home
```

Creates the user, `/opt/healthie{,/data,/data/snapshots}`, `.env` (root:root
0600) and the unit; enables but does not start it — there is no binary yet.
Re-runnable; never overwrites `/opt/healthie/.env`. Warns if the clock is not
NTP-synced, since every row is date-keyed.

## Deploy

```bash
just deploy dietpi@dietpi.home
```

Refuses a dirty tree, tests, cross-compiles, snapshots the DB, uploads beside the
running binary, swaps, then waits for `/api/health` to report **this** commit — a
healthy answer from the old binary does not pass.

**A modified `.beads/issues.jsonl` will block it.** Commit the export on `main`
per `CLAUDE.md`, or stash it for the deploy. Not a bug: `build.rs` uses the same
predicate, so a dirty tree really would build `<sha>-dirty` and fail the gate.

The `⚠️ healthie.local does not reach healthie` warning is expected until the
proxy step below; the service is proven healthy on :3005 regardless.

## Tokens

```bash
just token dietpi@dietpi.home ingest         # Health Auto Export
just token dietpi@dietpi.home mcp            # MCP clients
just token dietpi@dietpi.home mcp revoke
```

Plaintext prints once; only an argon2 hash is kept. Re-running rotates, and the
old token dies immediately.

**Don't call the binary's `token` subcommand by hand** — `--db-path` defaults to a
_relative_ path and the binary creates and migrates whatever that resolves to, so
running it from a home directory silently builds a second empty database and
prints a token the live service will never accept.

## Reverse-proxy name (different repo)

`home-proxy` owns `/etc/caddy/Caddyfile`; healthie's deploy never touches it.
Paste `deploy/healthie.caddyfile` into `home-proxy/deploy/Caddyfile`, commit, run
that repo's `just deploy`. The mDNS alias follows from the hostname in the block.

While in there: `kammerz.local` is on the box but missing from the repo copy, so
the next `home-proxy` deploy would delete its route.

## When something is wrong

```bash
just status   dietpi@dietpi.home    # unit state + /api/health, direct and proxied
just logs     dietpi@dietpi.home
just rollback dietpi@dietpi.home    # previous binary, restart, re-gate
```

Direct and proxied are reported separately because a proxy-only failure belongs
to `home-proxy`.

Failed unit after exhausting its restart budget (`StartLimitBurst=5`/300s):
`sudo systemctl reset-failed healthie`.

**Restoring the database is deliberately not a recipe.** A failed deploy prints
the exact command with the snapshot name. It destroys everything written since,
and it removes `-wal`/`-shm` — not optional, an old WAL replaying onto a restored
DB corrupts it. Snapshots: `/opt/healthie/data/snapshots/`, pruned to 10.

### `/mcp` 403s but everything else works

From `192.168.30.4` (the box's `eth0.30` VLAN address), `/api/health` and
`/ingest/hae` are fine and **only** `/mcp` 403s; a valid token doesn't help. rmcp
enforces a `Host:` allowlist on `/mcp` as a DNS-rebinding defense, and
`HEALTHIE_MCP_ALLOWED_HOSTS` deliberately omits that VLAN — it's a segmentation
boundary and healthie serves health data over plain HTTP.

To change it: add the address in `deploy/healthie.service` and redeploy. That
works _because the allowlist lives in the unit, not `.env`_ — see Configuration.

### Service won't start after a unit change

The unit is over-sandboxed relative to what the binary needs. Remove in this
order:

1. `RestrictAddressFamilies` — omits `AF_NETLINK`, which glibc's `__check_pf`
   needs for outbound name resolution. Nothing outbound today; the first HTTP
   client added breaks here, and the symptom is a DNS failure, not a sandbox
   error.
2. `ProtectSystem=strict` / `ReadWritePaths` — anything writing outside
   `/opt/healthie/data`.
3. `PrivateDevices`, `ProtectKernelTunables`, `ProtectKernelModules`,
   `ProtectControlGroups`.
4. `NoNewPrivileges`, `RestrictNamespaces`, `RestrictSUIDSGID`,
   `LockPersonality`, `RemoveIPC`.

`systemd-analyze security healthie` scores the result. CI parses the unit on
every change.

## Configuration

`.env`: `HEALTHIE_DB_PATH`, `HEALTHIE_LISTEN`. Unit: `RUST_LOG`,
`HEALTHIE_MCP_ALLOWED_HOSTS`.

⚠️ **`EnvironmentFile=` overrides `Environment=`**, unconditionally and
regardless of order — and `.env` is written once at bootstrap and never
overwritten. So a key set in `.env` pins itself permanently and every later unit
push silently does nothing. Keep `RUST_LOG` and `HEALTHIE_MCP_ALLOWED_HOSTS`
commented there unless you mean exactly that.

`HEALTHIE_MCP_ALLOWED_HOSTS` is not optional: rmcp defaults to localhost only, so
`/mcp` 403s everything arriving under any other name — including through the
proxy — while the service otherwise looks healthy.

## Security posture

Plain HTTP, bearer tokens as the only gate, remote access via the household VPN;
health data and tokens cross the network in cleartext. Binds `0.0.0.0` rather
than loopback because `.local` names generally don't resolve across the VPN, so
loopback-plus-proxy would make healthie unreachable off-LAN. Transport security
is tracked separately.

⚠️ Uncommenting the loopback line in `deploy/.env.example` and redeploying
**will not change the box** — that file only seeds `bootstrap`. Edit
`/opt/healthie/.env` there and restart, or delete it and re-bootstrap.
