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
