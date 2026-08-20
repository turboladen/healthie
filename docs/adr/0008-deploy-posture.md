# ADR-0008: Deploying to the ODroid — atomic swap, proven identity, and a proxy healthie does not own

- **Status:** Accepted
- **Date:** 2026-08-09
- **Related:** healthie-93n (this deploy kit), healthie-cnj (the reverse-proxy
  block, applied in the `home-proxy` repo), healthie-jo8 (transport security,
  deferred). ADR-0005 (the single-binary backend and kinded tokens this
  deploys). Code: `deploy/`, `Justfile` (`bootstrap`, `deploy`, `rollback`,
  `token`), `healthie-backend/build.rs`, `healthie-backend/src/api/health.rs`.

## Context

healthie has been a thing that runs on a laptop. M2 finished the single deployed
binary, so it now needs to run unattended on the household ODroid N2+ (DietPi,
aarch64, systemd 252) alongside four other services: fewd on :3000, chorez on
:3001, kammerz on :3002, and a lighttpd PHP stack behind a shared Caddy reverse
proxy that owns :80. healthie takes :3005, which is already the binary's
default.

Two sibling projects had solved most of this. chorez contributed the safety
machinery — a dirty-tree guard, an online pre-deploy snapshot, upload-then-swap
with the previous binary retained, a health wait. kammerz contributed the
cleanups — an idempotent bootstrap separate from deploy, `sudo -n` everywhere,
the cross-linker on the cargo line rather than in a committed `.cargo/config.toml`,
and a health gate that asserts the deployed commit.

What neither contributed, contrary to the assumption this work started from, was
Caddy configuration. Establishing that changed the shape of the kit.

## Decision

### 1. The deploy proves which binary answered, rather than that something answered

`GET /api/health` gains a `build` field carrying the git commit the binary was
compiled from. `just deploy` resolves the commit once and uses that single string
for both the build and the post-deploy assertion, matching `"build":"<sha>"`
including the closing quote.

- **Why this and not a status check:** a service that responds is not evidence
  that the _new_ service is responding. A `systemctl stop` that silently fails
  leaves the old process serving a perfectly healthy 200, and every subsequent
  deploy appears to succeed while shipping nothing.
- **Why exact and not prefix:** a prefix match accepts a `<sha>-dirty` binary
  when checking for `<sha>`, which makes the gate depend entirely on the
  dirty-tree guard holding. Exact costs nothing.
- **What this publishes:** `/api/health` needs no token, so the git sha of the
  running build is readable by anyone who can reach the port. That is
  deliberate — the gate gets its answer over plain `curl` on the box, with no
  credential to provision before the first deploy can prove itself — and the
  disclosure is negligible for a private repository on a LAN. It is recorded
  here rather than left unstated, because it is the kind of thing that stops
  being negligible if healthie is ever exposed beyond the LAN.
- **Why the commit is passed in rather than looked up twice:** cargo reruns a
  build script only when a declared input changes, and getting that declaration
  right for git is subtle enough to have been wrong twice. Watching `HEAD` alone
  never fires, because `HEAD` is a symbolic ref that a commit does not touch.
  Watching the resolved loose ref as well still misses a repository whose refs
  are packed — a fresh clone, or anything after `git gc` — because the loose file
  does not exist when the declaration is made, and the next commit creates it
  without touching `HEAD` or `packed-refs`. A stale hash there does not fail
  loudly: it fails a _good_ deploy and prints a rollback command. Passing the
  value in removes the class; the script's own git lookup, which now watches the
  reflog, remains only for a plain `cargo build`.

### 2. Bootstrap is separate from deploy, and only bootstrap creates anything

`just bootstrap` creates the user and group, the directories, `.env`, and the
unit — then stops without starting the service. `just deploy` assumes all of it
and refuses to run otherwise.

- **Why separate:** they have different failure modes and different frequencies.
  Folding provisioning into deploy means every deploy carries code that only
  matters once, and a first deploy that half-succeeds leaves a crash-looping unit
  that needs `systemctl reset-failed` before anything works again.
- **Why bootstrap does not start the service:** there is no binary yet. Starting
  would produce a crash-loop as the expected outcome of a successful bootstrap.
- **Why the group is created explicitly:** `useradd` creating a matching group is
  an `/etc/login.defs` setting, not a guarantee, and the unit names
  `Group=healthie`.

### 3. Ownership is part of the sandbox, not separate from it

`ReadWritePaths=/opt/healthie/data` lifts systemd's namespace restriction only;
ordinary file permissions still apply. `/opt/healthie{,/data,/data/snapshots}`
are `0755 healthie:healthie`, which is what lets the service create the database
and its WAL files, and what lets the pre-deploy snapshot run `sqlite3` as the
service user.

`/opt/healthie/.env` is `root:root 0600`. systemd reads `EnvironmentFile=` as PID
1 before dropping privileges, so the service user never needs to read it. This
departs from the other services on the box, which leave their env files owned by
the service user (one of them world-readable); the departure is deliberate.

### 4. `.env` holds only what an operator tunes; the unit holds what a deploy changes

`HEALTHIE_DB_PATH` and `HEALTHIE_LISTEN` live in `.env`. `RUST_LOG` and
`HEALTHIE_MCP_ALLOWED_HOSTS` live in the unit, and are commented out in
`.env.example`.

The split follows from a systemd rule that is easy to get backwards:
`EnvironmentFile=` settings **override** `Environment=`, unconditionally and
regardless of directive order. Combined with `.env` being written once at
bootstrap and never overwritten, a key set in `.env` pins itself permanently —
every later unit push becomes a silent no-op. For the MCP host allowlist
specifically that failure is invisible and total: `/mcp` returns 403 to
everything that is not localhost while the service looks entirely healthy, and no
redeploy can fix it.

`HEALTHIE_MCP_ALLOWED_HOSTS` must be set at all, because rmcp's Host allowlist
defaults to `localhost`, `127.0.0.1` and `::1` — so reaching healthie by any
other name, including through the reverse proxy, is rejected by default.

Which addresses it names was decided rather than defaulted: the `192.168.10.x`
address and the proxy names, but **not** the box's second address `192.168.30.4`
on `eth0.30`. That VLAN is a segmentation boundary, and §7's cleartext posture is
exactly what makes crossing it a decision rather than a convenience. The cost is
an asymmetric failure — from that VLAN only `/mcp` 403s while every other
endpoint works — which `docs/operations.md` documents so it is not debugged from
scratch.

### 5. Snapshot before, keep the previous binary, prune after

A `VACUUM INTO` snapshot into `data/snapshots/` runs while the service is still
up (a plain copy of a live WAL database is not consistent; `VACUUM INTO` is). The
swap keeps `healthie-backend.prev`. Pruning to ten keeps only after a healthy
deploy — a failed deploy is exactly when older snapshots matter most.

Snapshots live in a subdirectory rather than beside the database so that no
future restore procedure can write a `*.db` glob that also matches the live file.
Binary rollback is a recipe; database restore stays a printed command naming the
specific snapshot, because it destroys everything written since, and it includes
removing the `-wal`/`-shm` files — an old WAL replaying onto a restored database
corrupts it.

The application does **not** take its own pre-migration snapshot. It would
duplicate this one, and the only case it uniquely covers — a migration running on
a restart that is not a deploy — is already bounded by the crash-loop cap.

### 6. healthie does not configure the reverse proxy

`/etc/caddy/Caddyfile` is owned outright by the `home-proxy` repository, which
installs its own copy wholesale on deploy and publishes an mDNS alias per
declared hostname. healthie publishes `deploy/healthie.caddyfile` for a human to
paste there, and its own deploy never touches Caddy.

- **Why not auto-sync, which is where this started:** the belief that sibling
  projects auto-synced their blocks turned out to be false — they have no Caddy
  configuration at all. Two repositories writing one file with no coordination
  means each silently reverting the other. The shared file had _already_ drifted
  this way: a hand-added `kammerz.local` block exists on the box and not in
  `home-proxy`, so that repo's next deploy will delete it, leaving the mDNS name
  resolving but its route falling through to the catch-all.
- **What the deploy does instead:** probe `Host: healthie.local` and warn. The
  probe matches the build string rather than accepting any 2xx, because with no
  healthie block the request falls through Caddy's catch-all to lighttpd, which
  answers happily.
- **Why the warning is not a failure:** the proxy name is another repository's
  deploy. Failing healthie's deploy over it would report a healthy service as
  broken.

### 7. Plain HTTP on a trusted LAN, stated rather than assumed

healthie carries health data behind bearer tokens over unencrypted HTTP, matching
the rest of the fleet. Remote access is via the household VPN. The listener binds
`0.0.0.0` rather than loopback specifically because mDNS `.local` names generally
do not resolve across that VPN, so loopback-plus-proxy would make healthie
unreachable off-LAN. Transport security is deferred to healthie-jo8, and
`.env.example` carries the loopback-bind line ready to uncomment when it lands.

Note what uncommenting it does **not** do: `deploy/.env.example` is a template
for `bootstrap`, and `/opt/healthie/.env` is written once and never overwritten
by a deploy (§4). Editing the local copy and redeploying changes nothing on the
box. Switching the bind address means editing `/opt/healthie/.env` in place, or
deleting it and re-running `bootstrap`.

## Consequences

- A deploy cannot silently ship nothing; the failure modes that used to be silent
  now name themselves.
- A first deploy and a subsequent one differ in ways that are handled explicitly
  rather than by luck: no snapshot to take, no `.prev` to keep, a `systemctl
stop` that exits 5, and no tokens until they are provisioned.
- `healthie.local` does not work until healthie-cnj is applied in `home-proxy`.
  Every deploy says so until it is.
- The unit's sandboxing is untested against the box until Steve's first
  bootstrap. `RestrictAddressFamilies` is the contingent one: it omits
  `AF_NETLINK`, which glibc needs when resolving names for an outbound
  connection, so the first HTTP client added to this binary will break with what
  looks like a DNS failure rather than a sandbox error. It is first in the
  documented removal order.
- CI parses the unit file. That is the only place systemd ever sees it — local
  verification cannot, so a directive typo would otherwise reach the box.
