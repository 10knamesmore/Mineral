#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo clippy --locked --workspace --all-targets --release -- -D warnings
INSTA_UPDATE=no cargo t --locked --no-fail-fast
cargo dylint --all -- --locked --workspace --all-targets
