#!/bin/sh
# Proves the determinism and purity gates in clippy.toml are live: kf-core's
# canary module (compiled only with --cfg kf_canary) must be rejected by clippy
# for each disallowed call below. ROADMAP.md M1.
set -u

out=$(RUSTFLAGS="--cfg kf_canary" cargo clippy -p kf-core --locked -- -D warnings 2>&1) && {
    echo "FAIL: canary passed clippy; the gates in clippy.toml are not active"
    exit 1
}

status=0
for needle in 'method `f64::sin`' 'method `std::fs::read`' 'method `std::time::Instant::now`' 'type `std::time::Instant`'; do
    if printf '%s\n' "$out" | grep -qF "disallowed $needle"; then
        echo "ok: rejected disallowed $needle"
    else
        echo "FAIL: no rejection for disallowed $needle"
        status=1
    fi
done
[ "$status" -eq 0 ] || printf '%s\n' "$out"
exit "$status"
