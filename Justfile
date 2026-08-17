# The deploy target: an ODroid N2+ running DietPi, aarch64.
linux_target := "aarch64-unknown-linux-gnu"

# Local tool prerequisites (beyond stable cargo): `just`, `dprint`
# (brew install dprint), nightly rustfmt (rustup toolchain install nightly),
# and `jq` for the ci-before-push hook. A missing tool fails fmt-check — and
# therefore blocks pushes via the hook — with a command-not-found error.

# Format the whole repo: dprint (md/json/toml/yaml catch-all, per dprint.jsonc)
# + rustfmt (Rust; nightly because rustfmt.toml uses nightly-only options).
# Prettier is deferred until a frontend exists — add it here (and to fmt-check
# and CI's format job) when frontend/ lands.
fmt:
    dprint fmt
    cargo +nightly fmt --all

# Verify formatting without writing — mirrors CI's `format` job.
fmt-check:
    dprint check
    cargo +nightly fmt --all --check

# Mirrors the `backend` CI job: build + test + clippy, workspace-wide, --locked.
ci-backend:
    cargo build --workspace --locked
    cargo test --workspace --locked
    cargo clippy --workspace --all-targets --locked -- -D warnings -D clippy::pedantic

# Spell-check via typos-cli (brew install typos-cli). Skips with a warning when
# not installed locally; CI's `typos` job installs it and enforces hard.
typos:
    @if command -v typos >/dev/null; then typos; else echo "⚠️  typos-cli not installed — skipping (CI enforces)"; fi

# Local mirror of the CI gates that can run anywhere (format + backend); the
# aarch64 cross-check job is CI-only (needs the Linux cross toolchain), so
# local green does not guarantee that job. The ci-before-push hook
# (.claude/hooks/ci-before-push.sh) runs this before any `git push` /
# `gh pr create`.
ci: fmt-check ci-backend typos
    @echo "✅ just ci: local CI gates passed (format, backend, typos; aarch64 check is CI-only)"

# Prove that the GIT_COMMIT healthie-backend/build.rs embeds actually tracks
# HEAD — the property `just deploy`'s health gate depends on. A stale hash makes
# a good deploy fail the gate and print a rollback command.
#
# Deliberately NOT part of `just ci`: proving it requires making commits, and a
# check that commits into the repo it was invoked from would rewrite the
# developer's branch. Everything happens in a throwaway `git init` under a temp
# dir. Identity and signing are forced on the command line because commit
# signing is enabled globally here and the throwaway repo has no key.
verify-build-sha:
    #!/usr/bin/env bash
    set -euo pipefail

    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT

    # Probe the REAL build script, not a copy of its logic.
    mkdir -p "$work/src"
    cp healthie-backend/build.rs "$work/build.rs"
    printf 'fn main() { print!("{}", env!("GIT_COMMIT")); }\n' > "$work/src/main.rs"
    # `Cargo.lock` is gitignored alongside `target/`: cargo generates both during
    # the first build, and either one left untracked makes the tree permanently
    # dirty — build.rs would then append `-dirty` to every reading and the
    # assertions below would pass or fail for reasons that have nothing to do
    # with the rerun triggers.
    printf 'target/\nCargo.lock\n' > "$work/.gitignore"
    # An empty [workspace] table detaches the probe from any workspace cargo
    # might otherwise discover by walking up from the temp dir.
    printf '[workspace]\n\n[package]\nname = "build-sha-probe"\nversion = "0.0.0"\nedition = "2021"\n' \
        > "$work/Cargo.toml"

    cd "$work"
    g() { git -c user.name=probe -c user.email=probe@invalid -c commit.gpgsign=false "$@"; }
    g init -q .
    g add -A
    g commit -qm probe

    # Assert cleanliness BEFORE measuring anything.
    if [ -n "$(g status --porcelain)" ]; then
        echo "❌ the throwaway repo is dirty before measuring — every reading below" >&2
        echo "   would carry a '-dirty' suffix and prove nothing:" >&2
        g status --porcelain >&2
        exit 1
    fi

    read_build() { cargo build -q 2>/dev/null; ./target/debug/build-sha-probe; }
    expect() {
        if [ "$1" != "$2" ]; then
            echo "❌ $3: binary reports '$1', HEAD is '$2'" >&2
            exit 1
        fi
    }

    expect "$(read_build)" "$(g rev-parse --short=8 HEAD)" "initial build"

    # 1. Loose ref — the ordinary case.
    g commit -q --allow-empty -m second
    expect "$(read_build)" "$(g rev-parse --short=8 HEAD)" "after a commit on a loose ref"
    echo "  ✓ loose ref: a commit re-embeds the new sha"

    # 2. Packed refs — a fresh clone, or anything after `git gc`. Triggers are
    #    computed here while the ref is packed; the next commit creates a loose
    #    ref that a loose-ref-only trigger set would never have declared.
    g pack-refs --all
    read_build > /dev/null
    g commit -q --allow-empty -m third
    expect "$(read_build)" "$(g rev-parse --short=8 HEAD)" "after a commit on a packed ref"
    echo "  ✓ packed refs: a commit re-embeds the new sha"

    # 3. A caller-supplied GIT_COMMIT wins verbatim — this is the path `just
    #    deploy` uses to make build-time and gate-time the same string.
    GIT_COMMIT=deadbeef cargo build -q 2>/dev/null
    forced="$(./target/debug/build-sha-probe)"
    expect "$forced" "deadbeef" "with GIT_COMMIT set in the environment"
    echo "  ✓ env override: GIT_COMMIT is taken verbatim"

    # 4. …and unsetting it falls back to git rather than sticking.
    expect "$(read_build)" "$(g rev-parse --short=8 HEAD)" "after GIT_COMMIT was unset again"
    echo "  ✓ env override is not sticky"

    echo "✅ verify-build-sha: the embedded commit tracks HEAD"

# ---------------------------------------------------------------------------
# Deployment. Target is dietpi@dietpi.home (ODroid N2+, DietPi, aarch64).
#
# First time on a box:  just bootstrap dietpi@dietpi.home
# Every time after:     just deploy    dietpi@dietpi.home
#
# Everything here needs passwordless sudo for the remote user; `sudo -n` fails
# fast rather than hanging on a prompt. `scp` is never used — the remote login
# shell is fish, which breaks sftp — so files travel by `ssh ... tee` and
# scripts by `ssh ... bash -s`.
# ---------------------------------------------------------------------------

# Cross-compile the release binary for the box.
#
# The linker is set on the cargo invocation, NOT in a committed .cargo/config.toml:
# cargo applies a [target.<triple>] section to host builds too when the triples
# match, so committing one would break a native build on any aarch64 Linux box.
#
# One-time prereqs on a fresh macOS dev host:
#   brew tap messense/macos-cross-toolchains
#   brew install aarch64-unknown-linux-gnu
#   rustup target add aarch64-unknown-linux-gnu
build-linux:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v aarch64-unknown-linux-gnu-gcc > /dev/null 2>&1; then
        echo "❌ aarch64-unknown-linux-gnu-gcc not found on PATH — the cross linker is required." >&2
        echo "   Install the one-time prereqs (see the comment above build-linux):" >&2
        echo "     brew tap messense/macos-cross-toolchains" >&2
        echo "     brew install aarch64-unknown-linux-gnu" >&2
        echo "     rustup target add aarch64-unknown-linux-gnu" >&2
        exit 1
    fi
    # GIT_COMMIT is honored if the caller exported one (`just deploy` does, so
    # the build and the health assertion use the same string); build.rs falls
    # back to asking git when it is unset.
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-unknown-linux-gnu-gcc \
        cargo build --release --locked --target {{ linux_target }} -p healthie-backend

# First-time provisioning for a FRESH box — run ONCE, before the first deploy.
#
# `deploy` is a steady-state redeploy and assumes this has run: the healthie
# user, /opt/healthie{,/data,/data/snapshots}, /opt/healthie/.env and the unit
# all already exist. Without them its first upload fails with "No such file or
# directory" and the service crash-loops until `systemctl reset-failed`.
#
# Idempotent — safe to re-run; never clobbers an existing /opt/healthie/.env.
bootstrap host:
    #!/usr/bin/env bash
    set -euo pipefail

    if [ ! -f deploy/.env ]; then
        echo "❌ deploy/.env not found." >&2
        echo "   cp deploy/.env.example deploy/.env, review it, then re-run." >&2
        echo "   (It is gitignored. Nothing in it is secret today, but read the" >&2
        echo "    override warning at the top before uncommenting anything.)" >&2
        exit 1
    fi

    # Config travels as base64 positional arguments: single tokens with no
    # shell-significant characters, so nothing depends on quoting surviving both
    # the local shell and the remote one. macOS base64 does not wrap, GNU does;
    # strip newlines either way.
    env_b64="$(base64 < deploy/.env | tr -d '\n')"
    unit_b64="$(base64 < deploy/healthie.service | tr -d '\n')"

    ssh "{{ host }}" "sudo -n bash -s" -- "$env_b64" "$unit_b64" < deploy/bootstrap.sh

    echo ""
    echo "✅ bootstrapped {{ host }} — now run: just deploy {{ host }}"

# Deploy to the box: `just deploy dietpi@dietpi.home`.
#
# Atomic, snapshot-backed, and it proves what it deployed:
#   1. Refuse a dirty tree, then resolve the commit ONCE and use that one string
#      for both the build and the post-deploy assertion.
#   2. Refuse to run at all if bootstrap has not.
#   3. Snapshot the live database before touching anything.
#   4. Upload beside the running binary, then stop / swap / start.
#   5. Wait for /api/health to report THIS build. A service that answers is not
#      proof the new binary is the one answering.
#   6. Prune old snapshots, and only then.
#
# Gated on ci-backend: this ships to the only production box, and a deploy is
# rare enough to pay for the tests. It runs from inside the body rather than as
# a just dependency, because dependencies run BEFORE the body — as a dependency
# it would spend several minutes on the test suite only to then refuse a dirty
# tree. Cheap refusals first.
deploy host:
    #!/usr/bin/env bash
    set -euo pipefail

    # `git status --porcelain` — the SAME predicate build.rs uses to decide the
    # `-dirty` suffix, untracked files included. A `git diff` guard would be
    # tracked-only: an untracked file would pass here but ship a binary marked
    # `-dirty`, and the exact-match gate below would then fail for a reason
    # nobody would guess from the message.
    if [ -n "$(git status --porcelain)" ]; then
        echo "❌ working tree is dirty — commit or stash before deploying, so the" >&2
        echo "   deployed binary maps to a real commit." >&2
        git status --short >&2
        exit 1
    fi

    sha="$(git rev-parse --short=8 HEAD)"
    echo "==> Deploying commit ${sha} to {{ host }}"

    # /opt/healthie/.env is root-owned 0600, so this needs sudo to see it — a
    # plain `test -e` as the login user reports "missing" for a file that is
    # merely unreadable, which would send you off to re-run bootstrap for no
    # reason.
    if ! ssh "{{ host }}" "sudo -n test -e /opt/healthie/.env"; then
        echo "❌ /opt/healthie/.env not readable on {{ host }} — bootstrap has not run" >&2
        echo "   (or passwordless sudo is not configured, which everything here needs)." >&2
        echo "   Run: just bootstrap {{ host }}" >&2
        echo "   (Deploying without it fails on upload and crash-loops the unit.)" >&2
        exit 1
    fi

    just ci-backend
    GIT_COMMIT="$sha" just build-linux

    # The developer's clock, not the box's — only ever used to name a file, and
    # pruning sorts by mtime, so a skewed laptop cannot misorder the snapshots.
    ts="$(date +%F-%H%M%S)"
    snapshot="/opt/healthie/data/snapshots/pre-deploy-${ts}.db"

    # Three outcomes, and they must NOT be collapsed: `test -e` exits 0 for
    # present and 1 for absent, but ssh itself exits 255 when it cannot reach the
    # host. Treating 255 as "absent" would announce "no database yet", take no
    # backup, and then deploy a binary that migrates on boot — losing the only
    # data-safety net in this kit at exactly the moment it is needed. The window
    # is real: the last proof ssh works is the .env check above, and the test
    # suite plus the cross-compile run for minutes in between.
    set +e
    ssh "{{ host }}" "sudo -n test -e /opt/healthie/data/healthie.db"
    db_probe=$?
    set -e
    case "$db_probe" in
        0)
            echo "==> Snapshotting the database to ${snapshot} ..."
            # VACUUM INTO is a consistent ONLINE snapshot — safe while the
            # service is still up and writing, which a plain cp of a WAL
            # database is not.
            #
            # SC2029: ${snapshot} expanding on the client is deliberate — the
            # timestamp is chosen here so the same path can be printed in the
            # rollback hint below.
            # shellcheck disable=SC2029
            if ! ssh "{{ host }}" "sudo -n -u healthie sqlite3 /opt/healthie/data/healthie.db \"VACUUM INTO '${snapshot}'\""; then
                echo "❌ pre-deploy snapshot failed — aborting before anything changed." >&2
                exit 1
            fi
            ;;
        1)
            echo "==> No database yet — skipping the pre-deploy snapshot (first deploy)."
            snapshot=""
            ;;
        *)
            echo "❌ could not determine whether a database exists on {{ host }}" >&2
            echo "   (ssh exited ${db_probe}; 255 means the host was unreachable)." >&2
            echo "   Refusing to continue: proceeding would skip the pre-deploy backup" >&2
            echo "   and then deploy a binary that migrates on boot. Nothing has changed." >&2
            exit 1
            ;;
    esac

    # Uploaded while the old binary is still serving: downtime shrinks to the
    # restart, and a dropped stream cannot truncate the live binary. `> /dev/null`
    # because tee also writes to stdout — without it the whole binary comes back
    # down the wire into the terminal.
    echo "==> Uploading the binary ..."
    ssh "{{ host }}" "sudo -n tee /opt/healthie/healthie-backend.new > /dev/null" \
        < "target/{{ linux_target }}/release/healthie-backend"
    # tee creates 0644. Without this the swapped-in binary is not executable and
    # ExecStart fails with EACCES — a crash-loop, with .prev already moved aside.
    ssh "{{ host }}" "sudo -n chmod 0755 /opt/healthie/healthie-backend.new"

    # Every failure from here on leaves the box mid-deploy, so all of them get
    # the same exit: the rollback command, and the database restore when a
    # snapshot was taken.
    rollback_hint_and_exit() {
        echo "" >&2
        echo "↩️  ROLLBACK — restore the previous binary:" >&2
        echo "    just rollback {{ host }}" >&2
        if [ -n "$snapshot" ]; then
            echo "" >&2
            echo "    If the new build already migrated the database, ALSO restore the snapshot." >&2
            echo "    This DESTROYS anything written since the snapshot — read it before running it:" >&2
            echo "    ssh {{ host }} 'sudo -n systemctl stop healthie && sudo -n install -o healthie -g healthie -m 0644 ${snapshot} /opt/healthie/data/healthie.db && sudo -n rm -f /opt/healthie/data/healthie.db-wal /opt/healthie/data/healthie.db-shm && sudo -n systemctl start healthie'" >&2
            echo "    (the -wal/-shm removal is not optional: an old WAL replaying onto a" >&2
            echo "     restored database corrupts it)" >&2
        fi
        exit 1
    }

    echo "==> Stopping, swapping, starting ..."
    unit_b64="$(base64 < deploy/healthie.service | tr -d '\n')"
    # NOT left to `set -e`: remote-swap only restores .prev when the new binary
    # never landed, so a build that installs cleanly and then fails to start
    # leaves the broken binary in place with .prev beside it — needing exactly
    # the rollback command that a bare `set -e` abort would skip printing.
    set +e
    ssh "{{ host }}" "sudo -n bash -s" -- "$sha" "$unit_b64" < deploy/remote-swap.sh
    swap_status=$?
    set -e
    if [ "$swap_status" -ne 0 ]; then
        echo "" >&2
        echo "❌ the swap failed on {{ host }} (see above); the service may be down or" >&2
        echo "   running the new binary." >&2
        rollback_hint_and_exit
    fi

    echo ""
    echo "⏳ Waiting for /api/health on :3005 to report build ${sha} ..."
    set +e
    ssh "{{ host }}" "bash -s" -- "$sha" 3005 < deploy/health-wait.sh
    health_status=$?
    set -e
    if [ "$health_status" -ne 0 ]; then
        rollback_hint_and_exit
    fi

    # Soft: the reverse-proxy name lives in the home-proxy repo, not here, so a
    # missing route is someone else's deploy, not a failure of this one. The
    # service itself has already been proven healthy above.
    proxy="$(ssh "{{ host }}" "curl -fsS --max-time 3 -H 'Host: healthie.local' http://localhost/api/health" 2> /dev/null || true)"
    case "$proxy" in
        *"\"build\":\"${sha}\""*)
            echo "✅ http://healthie.local → build ${sha}"
            ;;
        *)
            # Matched on the build, never on a bare 2xx: with no healthie block
            # the request falls through Caddy's catch-all to lighttpd, which
            # answers perfectly happily and would satisfy a status-only check.
            echo "⚠️  http://healthie.local does not reach healthie yet — add healthie's"
            echo "    block to home-proxy/deploy/Caddyfile and deploy that repo."
            echo "    The service itself is healthy on :3005; this is the proxy name only."
            ;;
    esac

    # Only after a healthy deploy: a failed one is exactly when the older
    # snapshots matter most.
    echo "==> Pruning pre-deploy snapshots (keeping the 10 most recent) ..."
    ssh "{{ host }}" "sudo -n -u healthie bash -c 'cd /opt/healthie/data/snapshots && ls -1t pre-deploy-*.db 2>/dev/null | tail -n +11 | xargs -r rm -f'" || true

    echo ""
    echo "✅ Deployed ${sha} to {{ host }}"

# Provision (or revoke) a bearer token on the box, against the RIGHT database.
#   just token dietpi@dietpi.home ingest            # Health Auto Export
#   just token dietpi@dietpi.home mcp               # MCP clients
#   just token dietpi@dietpi.home mcp revoke
#
# A recipe rather than a documented command line, because the hand-typed version
# has a silent failure mode: --db-path defaults to the RELATIVE data/healthie.db
# and the binary creates and migrates whatever path that resolves to. Run from a
# home directory it builds a second, empty database, prints a token the live
# service will never accept, and reports success. The absolute path is baked in
# here so it cannot be forgotten — and it must be passed explicitly, since
# /opt/healthie/.env is root-owned and unreadable to the healthie user, so
# HEALTHIE_DB_PATH is not set for this invocation.
#
# Provisioning prints the plaintext exactly ONCE. It is never stored and never
# logged; only an argon2 hash is kept. Rotating replaces the previous token, so
# any client still using the old one starts getting 401s.
token host kind action='provision':
    #!/usr/bin/env bash
    set -euo pipefail
    # Bound to shell variables before use so the values are validated as data
    # rather than spliced straight into the remote command line.
    kind="{{ kind }}"
    action="{{ action }}"
    case "$kind" in
        ingest | mcp) ;;
        *)
            echo "❌ kind must be 'ingest' or 'mcp' (got '${kind}')" >&2
            exit 1
            ;;
    esac
    case "$action" in
        provision | revoke) ;;
        *)
            echo "❌ action must be 'provision' or 'revoke' (got '${action}')" >&2
            exit 1
            ;;
    esac
    # SC2029: client-side expansion is the intent — both values were validated
    # above against a closed set, so what reaches the box is one of four fixed
    # command lines.
    # shellcheck disable=SC2029
    ssh "{{ host }}" "sudo -n -u healthie /opt/healthie/healthie-backend \
        --db-path /opt/healthie/data/healthie.db \
        token --kind ${kind} ${action}"

# Put the previous binary back and restart. `deploy` keeps it as
# healthie-backend.prev on every swap.
#
# A recipe rather than a command to copy out of a failed deploy's output: this
# gets run when something is already broken, and that is the worst moment to
# retype a path. It covers the BINARY only — restoring the database is a
# separate, destructive step that deploy prints with the specific snapshot name,
# and it should stay something you read before you run.
rollback host:
    #!/usr/bin/env bash
    set -euo pipefail

    # Same three-way split as deploy's database probe: ssh's own 255 must not be
    # reported as "there is nothing to roll back to". No wrong action follows
    # either way, but this message gets read during an outage and it should not
    # send the operator looking for a missing file when the box is unreachable.
    set +e
    ssh "{{ host }}" "sudo -n test -f /opt/healthie/healthie-backend.prev"
    prev_probe=$?
    set -e
    if [ "$prev_probe" -gt 1 ]; then
        echo "❌ could not reach {{ host }} to check for a previous binary" >&2
        echo "   (ssh exited ${prev_probe}; 255 means the host was unreachable)." >&2
        exit 1
    fi
    if [ "$prev_probe" -ne 0 ]; then
        echo "❌ no /opt/healthie/healthie-backend.prev on {{ host }} — nothing to roll back to." >&2
        echo "   (Expected on a first deploy: there was no previous binary to keep.)" >&2
        exit 1
    fi

    echo "==> Restoring the previous binary on {{ host }} ..."
    ssh "{{ host }}" "sudo -n mv -f /opt/healthie/healthie-backend.prev /opt/healthie/healthie-backend && sudo -n systemctl restart healthie"

    echo "⏳ Waiting for /api/health ..."
    # The rolled-back binary's sha is whatever it is — we are deliberately not
    # asserting a specific build here, only that something healthy came back.
    if ssh "{{ host }}" "curl -fsS --max-time 5 --retry 10 --retry-delay 1 --retry-connrefused http://localhost:3005/api/health"; then
        echo ""
        echo "✅ rolled back on {{ host }} — the restored build is reported above."
        echo "   Note there is no .prev any more: rolling back again needs a deploy first."
    else
        echo "" >&2
        echo "❌ the restored binary is not answering either — inspect directly:" >&2
        echo "    just logs {{ host }}" >&2
        exit 1
    fi

# Service state and the build currently answering.
status host:
    #!/usr/bin/env bash
    set -euo pipefail
    ssh "{{ host }}" "sudo -n systemctl status healthie --no-pager" || true
    echo ""
    echo "--- /api/health (direct, :3005) ---"
    ssh "{{ host }}" "curl -fsS --max-time 5 http://localhost:3005/api/health" || echo "(no answer)"
    echo ""
    echo "--- /api/health (through the proxy, Host: healthie.local) ---"
    # A 404 here is the expected state before the home-proxy block exists: the
    # request falls through Caddy's :80 catch-all to lighttpd, which answers.
    # So "something replied" is not the same as "healthie replied" — match the
    # build string, the same test the deploy's soft probe uses.
    proxy="$(ssh "{{ host }}" "curl -sS --max-time 5 -w '\n%{http_code}' -H 'Host: healthie.local' http://localhost/api/health" 2>/dev/null || true)"
    code="$(tail -n1 <<<"$proxy")"
    body="$(sed '$d' <<<"$proxy")"
    # `000` is curl's code for "never got an HTTP response at all", and it is
    # non-empty — so it must be excluded explicitly, or nothing listening on :80
    # gets reported as a routing gap in a proxy that is not even running.
    if [[ "$body" == *'"build":"'* ]]; then
        echo "$body"
    elif [[ -n "$code" && "$code" != "000" ]]; then
        echo "⚠  $code from the :80 catch-all — healthie.local is not routed to healthie"
        echo "   (paste deploy/healthie.caddyfile into home-proxy and deploy that repo)"
    else
        echo "⚠  no answer on :80 — caddy down or host unreachable"
    fi
    echo ""

# Follow the service journal (Ctrl-C to stop).
logs host:
    ssh "{{ host }}" "sudo -n journalctl -u healthie -f --no-pager"
