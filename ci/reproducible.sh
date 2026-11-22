#!/bin/sh
# Builds the portable crates twice, from copies of the working tree in two
# different directories, and requires bit-identical artifacts for the host and
# wasm32 (STACK_DECISION.md "Reproducible builds"; ROADMAP.md M1).
set -eu

root=$(git rev-parse --show-toplevel)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
host=$(rustc -vV | sed -n 's/^host: //p')
cargo_home=${CARGO_HOME:-$HOME/.cargo}

for copy in a/first b/second-copy; do
    dir="$tmp/$copy"
    mkdir -p "$dir"
    (cd "$root" && git ls-files -co --exclude-standard -z | xargs -0 tar -cf -) | tar -xf - -C "$dir"
    for target in "$host" wasm32-unknown-unknown; do
        (cd "$dir" && RUSTFLAGS="--remap-path-prefix=$dir=/kineforge --remap-path-prefix=$cargo_home=/cargo" \
            cargo build --quiet --locked --release -p kf-core -p kf-dirgain --target "$target")
    done
done

status=0
for target in "$host" wasm32-unknown-unknown; do
    for lib in libkf_core.rlib libkf_dirgain.rlib; do
        a=$(sha256sum "$tmp/a/first/target/$target/release/$lib" | cut -d' ' -f1)
        b=$(sha256sum "$tmp/b/second-copy/target/$target/release/$lib" | cut -d' ' -f1)
        if [ "$a" = "$b" ]; then
            echo "ok: $target/$lib $a"
        else
            echo "FAIL: $target/$lib differs ($a vs $b)"
            status=1
        fi
    done
done
exit "$status"
