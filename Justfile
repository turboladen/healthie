fmt:
    cargo +nightly fmt --all

fmt-check:
    cargo +nightly fmt --all --check

ci:
    cargo build --workspace --locked
    cargo test --workspace --locked
    cargo clippy --workspace --all-targets -- -D clippy::pedantic
