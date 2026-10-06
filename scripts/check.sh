#!/usr/bin/env bash

set -euxo pipefail

cd "$(dirname "$0")/.."

rustup toolchain install stable --component rustfmt --component clippy --no-self-update
export INSTA_UPDATE=no

if [[ -n "${CHECK_TOOLING:-}" ]]; then
    (
        cd tooling/lints
        rustup toolchain install --no-self-update
        cargo fmt --check
        cargo clippy --locked --all-targets -- -D warnings
        cargo nextest run --locked --all-targets --no-fail-fast
        cargo doc --locked --no-deps --document-private-items
        cargo dylint --path . -- --locked --all-targets
    )
fi

cargo dylint --all -- --locked --workspace --all-targets
cargo fmt --all --check
stylua --check crates/
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo nextest run --locked --workspace --no-fail-fast

echo "DONE check"
