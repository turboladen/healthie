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

# Full local mirror of the GitHub Actions pipeline (.github/workflows/ci.yml).
# The ci-before-push hook (.claude/hooks/ci-before-push.sh) runs this before
# any `git push` / `gh pr create`. The aarch64 cross-check job is CI-only (it
# needs the Linux cross toolchain).
ci: fmt-check ci-backend
    @echo "✅ just ci: all CI gates passed (format, backend)"
