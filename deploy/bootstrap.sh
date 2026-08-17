#!/usr/bin/env bash
# First-time provisioning. Runs ON THE BOX as root, via
#   ssh <host> "sudo -n bash -s" -- <env_b64> <unit_b64> < deploy/bootstrap.sh
# (`bash -s` because the login shell is fish; config is base64 so no quoting
# has to survive two shells). Do not run directly.
#
# Idempotent; never overwrites an existing /opt/healthie/.env. Deliberately does
# not start the service — the first start belongs to `just deploy`.
set -euo pipefail

env_b64="${1:?missing base64 .env argument (run via: just bootstrap <host>)}"
unit_b64="${2:?missing base64 unit argument (run via: just bootstrap <host>)}"

app=healthie
root=/opt/healthie
unit=/etc/systemd/system/healthie.service

echo "==> Creating the ${app} system user and group..."
# Created explicitly rather than relying on useradd's group-per-user behavior,
# which is a /etc/login.defs setting rather than a guarantee. The unit names
# Group=healthie and fails to start if that group does not exist.
if ! getent group "$app" >/dev/null; then
    groupadd --system "$app"
fi
if ! getent passwd "$app" >/dev/null; then
    useradd --system --gid "$app" --shell /usr/sbin/nologin --no-create-home "$app"
fi

echo "==> Creating ${root}{,/data,/data/snapshots}..."
mkdir -p "$root/data/snapshots"
# ReadWritePaths= lifts systemd's namespace restriction only — DAC still
# applies. Without this the service can't create healthie.db and crash-loops,
# and the pre-deploy `sudo -u healthie sqlite3 ... VACUUM INTO` fails too.
chown -R "$app:$app" "$root"
chmod 0755 "$root" "$root/data" "$root/data/snapshots"

echo "==> Installing ${root}/.env..."
if [ -e "$root/.env" ]; then
    echo "    already present — leaving it untouched (it may hold hand-edited values)"
else
    # Decode to a temp path and move into place, so a truncated decode cannot
    # leave a partial .env sitting at the destination — which would then "exist"
    # and be skipped by every future run of this script.
    printf '%s' "$env_b64" | base64 -d > "$root/.env.incoming"
    mv -f "$root/.env.incoming" "$root/.env"
    echo "    created"
fi
# Asserted unconditionally, including on a re-run: the chown -R above would
# otherwise hand the file to the service user, and systemd reads EnvironmentFile=
# as PID 1 before dropping privileges, so the service never needs to read it.
chown root:root "$root/.env"
chmod 0600 "$root/.env"

echo "==> Installing the systemd unit..."
printf '%s' "$unit_b64" | base64 -d > "$unit.incoming"
mv -f "$unit.incoming" "$unit"
chmod 0644 "$unit"
systemctl daemon-reload
# Enable so a reboot brings healthie back. NOT started — there is no binary yet.
# stdout only: `set -e` aborts here if enable fails, and swallowing its stderr
# too would leave that abort with nothing to explain it.
systemctl enable "$app" >/dev/null
echo "    installed and enabled (not started — that is what \`just deploy\` does)"

# --- Clock sanity ---------------------------------------------------------
# Every row healthie stores is keyed by date. A box on the wrong timezone or
# with an unsynced clock files health data under the wrong date silently, and
# the error only becomes visible much later.
echo ""
echo "==> Checking the system clock..."
if command -v timedatectl > /dev/null 2>&1; then
    tz="$(timedatectl show -p Timezone --value 2> /dev/null || echo "")"
    synced="$(timedatectl show -p NTPSynchronized --value 2> /dev/null || echo "")"
    echo "    timezone: ${tz:-unknown}    NTP synchronized: ${synced:-unknown}"
    if [ "$synced" != "yes" ]; then
        echo "    ⚠️  The clock is NOT NTP-synchronized. Every metric healthie stores is" >&2
        echo "        keyed by date; a drifting clock files data under the wrong one:" >&2
        echo "          sudo timedatectl set-ntp true" >&2
    fi
else
    echo "    ⚠️  timedatectl not found — verify the timezone and clock sync by hand." >&2
fi

# --- sqlite3 ---------------------------------------------------------------
# The pre-deploy snapshot shells out to the sqlite3 CLI on the box, and a stock
# Debian/DietPi install does not have it. Nothing notices on the FIRST deploy —
# there is no database to snapshot yet — so the failure surfaces on the second
# one, after the test suite and the cross-compile have already run, as a bare
# "pre-deploy snapshot failed". Named here, once, while it is still cheap.
echo ""
echo "==> Checking for the sqlite3 CLI (used for the pre-deploy snapshot)..."
if command -v sqlite3 > /dev/null 2>&1; then
    echo "    present"
else
    echo "    ⚠️  sqlite3 not found. The FIRST deploy will still work; the second" >&2
    echo "        and every one after it will abort at the pre-deploy snapshot:" >&2
    echo "          sudo apt-get install -y sqlite3" >&2
fi

echo ""
echo "Bootstrap complete."
