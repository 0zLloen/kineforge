# ROADMAP — KineForge

Date: 2026-11-14. Ordered by dependency. Each milestone is small, measurable
and ends with explicit acceptance criteria; nothing after M0 starts until the
architecture is approved. "H1" etc. refer to hardware classes in
[PERFORMANCE_PLAN.md](PERFORMANCE_PLAN.md); "E*n*" to [EXPERIMENT_PLAN.md](EXPERIMENT_PLAN.md).

## Track map

| Requested track | Milestones |
|---|---|
| Foundational research | M0 |
| Cross-target Rust/WASM CI builds | M1 (and every later milestone) |
| Core libraries | M2, M3 |
| Telemetry | M4 |
| Simulation | M5 |
| Browser input & GPU capability detection | M6b, M8 |
| Minimal prototype | M7 (native slice) + M8 (web slice) |
| Rendering | M7 |
| Web version on static hosting | M8 |
| Browser telemetry export/import & local-first persistence | M4 (format), M8 (OPFS, export/import) |
| Analytics | M9 |
| Aim training | M10 |
| Calibration | M11 |
| Measurement tooling | M12 |
| Release | M13 |
| Future extensions | §F |

---

## M0 — Foundational research ✅ (this deliverable)

Acceptance: the ten documents exist; decisive claims verified against primary sources; open questions listed with verification procedures; **your review and approval of STACK_DECISION.md and ARCHITECTURE.md** (approved 2026-11-22); answers to STACK_DECISION.md §6 — items 2–8 still open, each needed only by the milestone that uses it (web accessibility and hosting: M8; Linux input permissions: M6a research mode; name: before any public repo or package; toolchain install: M1; Aimlabs: AL-E experiments only; ODD questions: M2/M11).

## M1 — Toolchain, workspace, cross-target CI ✅

**Status 2026-11-22: ✅ done.** Toolchain installed (rustup, Rust 1.100.0); workspace, lint gates, canary, reproducibility check, cargo-deny policy and CI workflow in place. First CI run on GitHub (`0zLloen/kineforge`, run 45029978914, commit `bc0fd9d`) green on all six runners (`ubuntu-24.04`, `ubuntu-24.04-arm`, `windows-2025`, `windows-11-arm`, `macos-15`, `macos-15-intel`) plus the wasm32/no_std/canary/reproducibility job and cargo-deny.

- With your permission: install rustup and the pinned toolchain (+ `wasm32-unknown-unknown`, `thumbv7em-none-eabihf` targets).
- Git repository, workspace with `kf-core` and `kf-dirgain` only (empty APIs are not created ahead of need).
- CI on `ubuntu-24.04`, `ubuntu-24.04-arm`, `windows-2025`, `windows-11-arm`, `macos-15` (arm64), `macos-15-intel`: fmt, clippy (incl. `disallowed_methods` for non-deterministic float functions in core crates), tests; `cargo check --target wasm32-unknown-unknown` for portable crates; `--target thumbv7em-none-eabihf` for `kf-dirgain`; cargo-deny (licence allow-list + advisories).

**Acceptance:** CI green on all six runners; a deliberate `f64::sin` call in `kf-core` fails clippy; a deliberate `std::fs` use in `kf-core` fails clippy (`disallowed_types`; `cargo check` for wasm32 would not catch it); a reproducibility job builds `kf-core`/`kf-dirgain` (native + wasm) twice in different directories and the SHA-256s match.

## M2 — `kf-dirgain`: bit-exact Open Directional DPI port ✅

**Status 2026-11-22: ✅ done.** All acceptance checks pass locally and in CI (run 45054329233, commit `fa66602`, green on all six native runners, the wasm32/oracle job and cargo-deny). Zero mismatches against the C engine (ODD `c177d23`) across 2592 scale vectors, 18 ported unit tests, 118 generated traces / 3909 steps, and 10⁸ compared fuzz ops + 10⁷ profile buffers; the same fixtures pass on wasm32-wasip1 under Node; `kf-dirgain` is `no_std`, zero-dependency and has no `unsafe`. The oracle lives in a separate native-only crate, `kf-dirgain-oracle` (vendored C, `gen-traces`, fuzzer), rather than a dev feature, so `kf-dirgain` keeps no build script.

- Port engine, profile (de)serialisation, CRC, capabilities (≈350 lines), `#![no_std]`, no allocation.
- Fixtures: all 2592 `scale_vectors.json` cases, the 17 C unit tests, the CRC-16 check value; **new stateful trace vectors** from a C generator (window lengths 0/1/16/31/32/64/128, every flag, 2×2, carry at ±32767/−32768, idle thresholds incl. u32 wrap, DPI switches, HID request/response pairs) — kept in KineForge unless you choose to adopt them in ODD.
- Native-only differential fuzzer against the C engine (`kf-dirgain-oracle` crate, C compiled with the `cc` crate).
- Moved to M11: the JSON ↔ bytes fixtures (OpenMouse tests 11/19) and the QMK raw-HID framing test, since profile JSON export and the `odd-hid` transport live there.

**Acceptance:** 100 % fixture pass on native, wasm32 (Node runner) and thumbv7em build; ≥10⁸ fuzz steps with zero state mismatches; no `unsafe`.

## M3 — `kf-core`: units, provenance, sensitivity mathematics ✅

**Status 2026-11-22: ✅ done.** All acceptance checks pass locally and in CI (run 45064106300, commit `fa4727c`: green on all six native runners, the wasm32/oracle job and cargo-deny; the golden hash is identical on all seven targets). Hand-computed fixtures within 2–4 ulps of 50-digit references (0.022°/count, sens 1, 800 CPI → 51.95 cm/360; Source 90° @ 4:3 → 106.26° @ 16:9; monitor distance 0 %/50 %/100 %; profile conversions such as VALORANT 0.4 → Overwatch 2 = 140/33); seeded round-trip properties; a golden hash of ≈320 000 results is bit-identical natively (debug and release) and on wasm32-wasip1. Deviations: **no `Counts` newtype** — motion values are source units × `value_scale` and are counts only when the source declares `units: counts`, so a counts type on motion fields would mislabel processed movement (`Nanos`, `Radians`, `Degrees`, `Aspect`, `ValueScale` instead); `kf-core` depends on `libm =0.2.16` for `tan`/`atan`; `InputEvent` mirrors the on-disk `input_*`/gap records (the per-event `Stamp` sketch was dropped: clock, resolution and meaning are per-source declaration facts); the other record types (`FrameRecord`, `Sim*`, `SceneView`, …) arrive with M4/M5/M7, which produce them. The profile table carries only what AIMLABS_RESEARCH.md §4 establishes (Source and Quake III VERIFIED from source at pinned commits, four COMMUNITY-ESTABLISHED yaw constants, the rest UNKNOWN) and is pinned to it by a test.

- Newtypes (counts, ns, radians), `InputLevel`/`EvidenceClass` (data provenance)/`ClaimStatus` (labels of constants)/clock-domain enums, `InputEvent`.
- Sensitivity: yaw/pitch per count, cm/360, FOV h↔v, Hor+/Vert− aspect scaling, monitor-distance matching (0 %…100 %), conversion between profiles; a profile table where every constant carries its `claim_status` and source.

**Acceptance:** hand-computed fixtures (e.g. 0.022°/count, sens 1, 800 CPI → 51.95 cm/360; Source 90° @ 4:3 → 106.26° @ 16:9); property tests (round-trip conversions); identical results native vs wasm32.

## M4 — Telemetry (`kf-telemetry`, `kf-store` native, `kf-cli` v0) ✅

**Status 2026-11-23: ✅ done.** All acceptance checks pass locally and in CI (run 45153293362, commit `45fa8eb`: green on all six native runners, the wasm32/oracle/KFLOG-parity job and cargo-deny; the golden logs read identically on all seven targets; the long fuzz with overflow checks and the Python parity run in CI). N9a on H1 (30 min, 3 × 8000 events/s + 1000 frames/s + 1000 `sim_camera`/s): 43.2 M events and 1.8 M frames recorded, zero drops, 0.37 % of one core (recorder + writer + sync threads, from `schedstat`), 35.4 B/event, the log validates. N9b: SIGKILL at a random point of the 100 ms flush cycle loses 0.1–95 ms, within 100 ms + the reported backlog; recovery writes the sidecar and leaves `raw.kflog` untouched. N9c is **modelled, not measured**: crash images cut at the writer's durable length with zero, garbage and partial tails recover with at most 1053 ms lost against the 1101 ms bound (`fsync_ms` + `chunk_flush_ms` + backlog); a VM hard reset or `dm-log-writes` replay needs root or a VM and has not been run. Fuzzing: 10⁶ mutated logs and `.kfz` archives (release, overflow checks on) plus 5000 per test run natively and on wasm32: no panics, memory and CPU stay linear. The golden logs dump byte-identically natively and on wasm32-wasip1, and `tools/read_kflog.py` matches the Rust dumps and sidecar rules. An adversarial review confirmed 44 of 58 findings, all fixed with regression tests (among them CRLF checkouts on Windows, a validator memory blow-up, quadratic gap matching, drain/switch order within a group, 16× reader buffering). Deviations: the space reserve is a 64 KiB file of random bytes, not `fallocate`; `kf-store` is a pure byte sink and the rollback after a writer drop happens in the recorder (`Recorder::writer_lost`); a memory-cap drop ends the session with `storage_error`; non-finite floats are refused when encoding; each `fdatasync` starts early by the previous one's duration so a write is durable within `fsync_ms`; `.kfz` members have flat names (`<table>.parquet`), `clock_sync` is exported and `trials`/`metrics` arrive with M9; cargo-deny now allows BSD-2-Clause (`zstd-sys`) and CC0-1.0 (`tiny-keccak`); the N9b test runs on Unix only. Open: a "share benchmark" export without `raw.kflog` needs a spec decision.

- KFLOG v1 writer/reader/validator/recovery; session header/end; Parquet export (native); stored-ZIP `.kfz` export/import; `FORMAT.md`; stdlib-only `read_kflog.py`.

**Acceptance:** PERFORMANCE_PLAN N9a (3 sources × 8000 events/s + frames + sim records, zero drops, ≤1 % core), N9b (SIGKILL loss ≤ 100 ms + writer backlog; recovery via sidecar, raw file untouched) and N9c (power-loss simulation ≤ 1.1 s) on H1; untrusted-input fuzzing of the reader/validator (malformed lengths, CRCs, ZIP members) finds no panics; a file written on Linux reads identically on Windows/macOS/wasm (hash); Python reader output equals Rust reader output on golden files.

## M5 — Simulation core (`kf-sim`)

- Fixed-tick clock (1 kHz default), camera from integer count totals, own xoshiro256++ + samplers, analytic target trajectories, ray–sphere/box hit tests, static-flick scenario, scoring model v1, replay driver.

**Acceptance (E2, E10):** replaying a corpus at simulated frame rates 30–1000 Hz gives identical camera state; per-tick state hashes identical on x86-64 Linux, aarch64 macOS (CI) and wasm32; golden RNG vectors stable.

**Status 2026-11-26: implemented; every acceptance check passes locally, CI pending.** E2: three corpus sessions played by a deterministic aiming bot — 1 kHz counts; 8 kHz with a directional-gain profile and pitch-clamp slams; `value_scale` 1024 with three concurrent targets, lifetimes and a time limit; all with absolute packets, wheel notches and right clicks that must not count — replayed at 30, 60, 144, 165, 240, 360, 500 and 1000 Hz and three random batchings (0.3–45 ms): the final camera **and every `sim_camera`/`sim_target`/`sim_shot` record** are bit-identical (stronger than the camera-only criterion: events interleave with ticks at their exact simulation time), and the unit totals equal the per-event camera and, without gain or clamping, the plain sums. E10: seven corpus sessions (including pause, `switch_source`, restart, abort, `next_block` and a 600 ms capped stall) pin the chained per-tick state hash and a record digest; identical in debug and release natively and on wasm32-wasip1; `kf-sim` joins the two-copy reproducible-build check. Golden RNG vectors: xoshiro256++ and splitmix64 equal the published C reference (compiled locally) and rand_xoshiro's published values; the samplers (including the unbiased integer sampler's rejection path) and the first static-flick target are pinned. Replay: `kf-cli replay` re-simulates a log group by group and compares every `sim_control`/`sim_camera`/`sim_target`/`sim_shot` record bit for bit; a live session recorded through kf-telemetry's `Recorder` (input logged before the frame that drains it) validates and replays identically, with and without gain; mismatches of every kind, corrupt and truncated logs and malformed logs are reported, not called identical. N10: a 300 s session (42 858 frames) replays in 0.11 s ≈ 2 700× real time on H1, one core, including log decoding (target ≥ 100×; CI runs it). A five-lens design review produced the second design revision; an adversarial code review confirmed 34 of 40 findings, all fixed with regression tests, and a second pass re-verified every fix and fixed its 11 new findings; every test-gap mutation is now caught. Deviations: no `glam` (explicit `[f64; 3]` helpers); events interleave with the frame's ticks (DATA_SPECIFICATION.md §2.1, §2.3, §9.1 amended), so scores do not depend on frame rate; shots only from the left button while a trial runs; `sim_shot.target` is the reference target (hit, else nearest by angle); `sim_hz` ≤ 10 000 and a frame steps at most `sim_hz` ticks; static flick v1 has spheres only, `concurrent` ≤ 64, a replacement per kill or timeout and no interval spawning; `kf.flick.v1` = ⌊100 × kills/s × accuracy⌋ per complete attempt; directional gain runs with the sensor DPI unknown (0) and one engine call per record that fits `i16`. With M10: on-target ticks, time to acquire, capsule hit tests, interval spawning, the `experiment` module (`next_block` is accepted with no effect). New recorder obligations for M7 (§2.3 item 1, §5.2, §9.1): cut a group before a frame message with `switch_source`, log input before forwarding it, write `sim_queue_overflow` before the next frame message. N5 (zero allocations) is not measured here: a counting allocator needs `unsafe`, which the workspace forbids, so M7 must choose a vetted dev-dependency or a test crate with its own lint exception.

## M6 — Input capture spikes (`kf-input`)

**M6a native:** Linux (evdev with permission; permission-free X11 XI2 raw / Wayland relative-pointer path), Windows Raw Input thread (`GetRawInputBuffer`, QPC), macOS (path chosen per STACK_DECISION §4); capability detection; optional hidraw L1 ground-truth backend (already readable on this machine).

**M6b browser:** Pointer Lock with `unadjustedMovement` (recorded result; a new source per lock acquisition; no silent fallback), exactly one motion event type per session with leaf-only coalesced events, lock-lifecycle annotations, COI detection.

Both: nothing on the capture path blocks — per-source rings with atomic drop accounting, recorder thread, non-blocking sim ring and per-frame record messages (ARCHITECTURE.md §3) — OS-specific thread priorities recorded, `RIDEV_NOLEGACY` only during captured trials on Windows.

**Acceptance (E0, E1):** clock characterisation fixtures for every path on H1; with uinput (available via ACL) and, if acquired, an RP2040 HID count source: Σ counts exact for every `os_raw_counts` path, scaling/acceleration detected and labelled for others; provenance in every session header.

## M7 — Native vertical slice (minimal prototype, part 1)

- winit window + thin wgpu renderer (instanced targets, crosshair, environment), egui HUD/settings, present-mode selection, `desired_maximum_frame_latency = 1` default, frame statistics recorded; static-flick scenario end to end with KFLOG recording.

**Acceptance:** N1, N2 (Fifo), N5 (headless input→sim→encode loop) on H1 @ 165 Hz, N6 measured; EXPERIMENT_PLAN E11 (render–sim geometric consistency) passes on Vulkan (and DX12/Metal in CI); session validates and replays bit-identically (drain-faithful) in `kf-cli`.

## M8 — Web vertical slice on static hosting (minimal prototype, part 2)

- Same scenario in the browser; WebGPU with WebGL2 fallback (retry adapter once); capability report page; COOP/COEP/CSP via `_headers` on the chosen host; OPFS log via a plain JS Worker; `persist()`; export (`showSaveFilePicker` or Blob) and import; achieved-rAF display; service worker for offline (updates between sessions only).

**Acceptance:** W1 size measured (single vs dual artifact decided on data) and ≤ ceiling; W2/W3 load times; Playwright smoke in CI (WebGPU-SwiftShader + WebGL2: render, readback hash, OPFS write/read, export); a session exported from Chromium and Firefox replays bit-identically in native `kf-cli`; EXPERIMENT_PLAN E11 passes on WebGPU-SwiftShader and WebGL2; the degradation matrix (WEB_PLATFORM_RESEARCH.md §2.9) is implemented and W10 (quota exhaustion) passes; browser experiments X-B1, X-B5, X-B7 run and recorded.

## M9 — Analytics v1 (`kf-analytics`)

Flick segmentation, endpoint error (along/across), overshoot, submovements, reaction time, Fitts throughput (W_e = 4.133·SD), tracking RMS error / time-on-target / lag, directional bias with CIs.

**Acceptance:** hand-computed and synthetic-trajectory fixtures with known answers; identical results native vs wasm (declared tolerance for float kernels); derived Parquet tables carry provenance metadata.

## M10 — Aim trainer v1

Scenario file format; families: static flick, dynamic flick (moving targets), centre-out, switching, reflex, precision, smooth tracking, reactive tracking, with shared pacing parameters (AIMLABS_RESEARCH.md §5 — KineForge's own designs); configurable scoring models over score-eligible metrics only; the `kf-sim` `experiment` module (seeded condition/block schedules, ABBA/Latin-square, warm-up flags, block annotations, one linked session per block, header `experiment` object); egui menus/results; benchmark mode with fixed seeds.

**Acceptance:** every family and every experiment schedule reproducible from seed (hash test); scoring fixtures; usability pass on native and web; N1/N2 hold in the heaviest scenario.

## M11 — Calibration (Open Directional DPI)

Port the authoritative calibration analysis (frozen with fixtures from real logs first), fitting/validation (E8), profile comparison, export as ODD JSON + 28-byte profile (with the JSON ↔ bytes fixtures from OpenMouse tests 11/19, moved from M2), upload via `kf-input` feature `odd-hid` (hidapi/hidraw natively, WebHID on Chromium; with the QMK raw-HID framing test, moved from M2).

**Acceptance:** analysis output equals the frozen fixtures; exported profiles byte-equal ODD's serializer; held-out validation report generated; **profile export refused for sources outside the calibration-validity rule** (EXPERIMENT_PLAN E8; e.g. accelerated browser streams); E8 step 0 (hidraw vs browser fit comparison) reported; firmware round-trip on the Glorious mouse (with your consent; device writes are irreversible-ish operations).

## M12 — Measurement tooling

Photodiode + microcontroller rig firmware and host tool; E5 latency and E6 pacing report generators; programmable HID count source firmware.

**Acceptance:** rig self-test (LED loopback) shows ≤0.1 ms timing error; baseline N11 latency numbers recorded per configuration.

## M13 — Packaging and release

cargo-dist release workflow; .app/.dmg (signed + notarised on macOS runners), MSI (signed), deb/AppImage; web deploy; licence notices (cargo-about); SBOM.

**Acceptance:** clean-machine install tests per OS; web deploy passes the smoke suite against production headers; unsigned release artifacts rebuild bit-identically and their SHA-256s are published next to the SBOM.

## F — Future extensions (each only when a concrete need exists)

- Standalone sensitivity converter (web + CLI) — `kf-core` only.
- Mouse analyzer without 3D — `kf-input` + `kf-analytics`.
- Latency research tool — rig + minimal renderer.
- Flick/tracking analyzers and a data-visualisation app (Parquet + DuckDB/Polars, or uPlot dashboards on the web).
- Experiment environment app (UI and study management on top of `kf-sim::experiment`; pre-registration workflow).
- DOM accessible web UI (strategy C) if web accessibility becomes a requirement.
- Python bindings or a C ABI only if a consumer needs live calls (data interchange covers most needs).
- Firmware calibration/validation tools; MCU reuse of `kf-dirgain`.
- Wasm threads once stable Rust supports them without nightly.
- Sharable, non-sensitive benchmark exports.
