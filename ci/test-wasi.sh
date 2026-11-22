#!/bin/sh
# Runs the kf-dirgain tests (C unit-test port, golden vectors) on
# wasm32-wasip1 under Node's built-in WASI. Extra arguments go to cargo test.
set -eu

root=$(git rev-parse --show-toplevel)
export CARGO_TARGET_WASM32_WASIP1_RUNNER="node --disable-warning=ExperimentalWarning $root/ci/wasi-node.mjs"
exec cargo test -p kf-dirgain --locked --target wasm32-wasip1 "$@"
