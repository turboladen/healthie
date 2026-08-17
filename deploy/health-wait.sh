#!/usr/bin/env bash
# Wait for /api/health to report the build just deployed. Runs ON THE BOX via
#   ssh <host> "bash -s" -- <sha> <port> < deploy/health-wait.sh
#
# Exit codes:
#   0  healthy, expected build
#   1  never answered, or the unit failed
#   2  answered with a DIFFERENT build — swap didn't take, or the stop failed
#      silently and the old process is still serving
# The caller treats every non-zero the same (deploy failed, offer a rollback);
# the distinction is carried by the message this script writes to stderr.
set -euo pipefail

sha="${1:?missing sha argument}"
port="${2:-3005}"

# The budget has to cover a slow FIRST start, not a healthy one: migrations run
# before the listener binds, so until they finish the port refuses and each pass
# costs only the sleep. 240 passes is ~2min of that — enough for a schema change
# over years of daily_metric rows on an ODroid, and the cost of being wrong is
# telling the operator to roll back a deploy that actually worked. Waiting longer
# does not slow a genuine failure down: is-failed gives a definite verdict as
# soon as systemd gives up (RestartSec=5 x StartLimitBurst=5, ~20-25s).
attempts=240
failed_unit=0

for _ in $(seq 1 "$attempts"); do
    if systemctl is-failed --quiet healthie; then
        failed_unit=1
        break
    fi
    if out="$(curl -fsS --max-time 2 "http://localhost:${port}/api/health" 2> /dev/null)"; then
        # Matched with the closing quote so a prefix cannot satisfy it — a
        # `<sha>-dirty` build must not pass a check for `<sha>`.
        case "$out" in
            *"\"build\":\"${sha}\""*)
                echo "$out"
                exit 0
                ;;
            *)
                echo "!! healthie is up but reports a different build than ${sha}:" >&2
                echo "   ${out}" >&2
                exit 2
                ;;
        esac
    fi
    sleep 0.5
done

if [ "$failed_unit" = 1 ]; then
    echo "!! the healthie unit entered a failed state — systemd has stopped retrying" >&2
else
    echo "!! no healthy answer for build ${sha} on :${port} before the wait elapsed" >&2
fi
# Diagnostics go to stderr so the caller's stdout carries only the health body.
# stderr already lands there, so redirecting stdout is all that is needed.
echo "--- systemctl status ---" >&2
sudo -n systemctl status healthie --no-pager >&2 || true
echo "--- last 50 journal lines ---" >&2
sudo -n journalctl -u healthie -n 50 --no-pager >&2 || true
exit 1
