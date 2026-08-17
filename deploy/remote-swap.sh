#!/usr/bin/env bash
# Stop, swap the binary, start. Runs ON THE BOX as root, via
#   ssh <host> "sudo -n bash -s" -- <sha> <unit_b64> < deploy/remote-swap.sh
# (`bash -s` because the login shell is fish). Expects the new binary already at
# /opt/healthie/healthie-backend.new. Do not run directly.
#
# -E so functions inherit the ERR trap.
set -Eeuo pipefail

sha="${1:?missing sha argument (run via: just deploy <host>)}"
unit_b64="${2:?missing base64 unit argument (run via: just deploy <host>)}"

root=/opt/healthie
bin="$root/healthie-backend"
unit=/etc/systemd/system/healthie.service

# Deliberately does NOT roll back a binary that started and then misbehaved —
# when `systemctl start` fails, $bin is present and executable so the restore is
# skipped. That call belongs to the health gate and `just rollback`.
#
# `|| true`: bash won't re-enter an ERR trap from inside the handler, so a
# failure here would abort under `set -e` and skip the start — the one outcome
# this exists to prevent.
rollback_swap() {
    echo "!! swap interrupted — putting the previous binary back if it moved, then starting" >&2
    if [ ! -x "$bin" ] && [ -x "$bin.prev" ]; then
        mv -f "$bin.prev" "$bin" || true
    fi
    systemctl start healthie || true
    exit 1
}

if [ ! -f "$bin.new" ]; then
    echo "❌ $bin.new is missing — the upload did not land." >&2
    exit 1
fi

# Re-pushed every deploy so unit changes reach the box. Note `daemon-reload`
# does NOT validate — systemd ignores unknown directives, so a typo is silent
# and the `unit-file` CI job is what catches it. Temp-then-move because a
# truncated decode over a running service's unit breaks it until next reboot.
printf '%s' "$unit_b64" | base64 -d > "$unit.incoming"
mv -f "$unit.incoming" "$unit"
chmod 0644 "$unit"
systemctl daemon-reload

# Signals too: a dropped ssh in the stop→start window would otherwise leave the
# service stopped.
trap rollback_swap ERR HUP INT TERM

# `|| true` for the first deploy, where the unit has never run (stop exits 5).
# A later silent stop failure is caught by the build assertion downstream.
systemctl stop healthie 2>/dev/null || true

kept_prev=0
if [ -f "$bin" ]; then
    mv -f "$bin" "$bin.prev"
    kept_prev=1
fi
mv -f "$bin.new" "$bin"
chown healthie:healthie "$bin"
chmod 0755 "$bin"

systemctl start healthie

trap - ERR HUP INT TERM
if [ "$kept_prev" = 1 ]; then
    echo "    swapped to ${sha} (previous binary kept at ${bin}.prev)"
else
    echo "    installed ${sha} (first deploy — no previous binary to keep)"
fi
