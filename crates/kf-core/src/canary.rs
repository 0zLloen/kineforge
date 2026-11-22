//! Compiled only with `--cfg kf_canary` by `ci/clippy-canary.sh`, which
//! requires clippy to reject every item here. Keeps the determinism and
//! purity gates in `clippy.toml` from silently going stale.
#![allow(dead_code)]

fn transcendental(x: f64) -> f64 {
    x.sin()
}

fn filesystem() -> std::io::Result<Vec<u8>> {
    std::fs::read("canary")
}

fn wall_clock() -> std::time::Instant {
    std::time::Instant::now()
}
