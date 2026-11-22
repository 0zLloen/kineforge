//! KineForge foundation: units, provenance enums, plain-data record types and
//! sensitivity mathematics (ARCHITECTURE.md §2). Builds for every target
//! including `wasm32-unknown-unknown`; OS, clock and non-deterministic float
//! APIs are rejected by the workspace `clippy.toml`.

#[cfg(kf_canary)]
mod canary;
